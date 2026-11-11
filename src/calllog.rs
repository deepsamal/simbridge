//! Per-call JSONL log (`SIMBRIDGE_LOG`) and tool allowlist (`SIMBRIDGE_TOOLS`).
//!
//! The log is the simulator side of a benchmark run: one line per tool call
//! with its arguments, outcome, cost and virtual time, plus a Unix timestamp so
//! it can be joined with the agent's own transcript (tokens, context size,
//! shell and edit calls), which simbridge cannot see.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rmcp::model::CallToolResponse;
use serde_json::{json, Value};

/// Feedback conditions from the benchmark design: each adds tools to the last.
/// C0 (compile only) and C1 (pass/fail) don't run simbridge at all.
pub const CONDITIONS: &[(&str, &[&str])] = &[
    ("C2", &["load", "reset", "run_for", "run_until", "serial"]),
    ("C3", &["load", "reset", "run_for", "run_until", "serial", "regs", "mem", "svd"]),
    (
        "C4",
        &[
            "load", "reset", "run_for", "run_until", "serial", "regs", "mem", "svd",
            "peripheral_trace", "snapshot", "restore",
        ],
    ),
];

/// Parse `SIMBRIDGE_TOOLS`: tool names and/or condition names (`C2`, `C3`,
/// `C4`), separated by commas or spaces; `all` or unset = every tool.
/// Unknown names are an error, so a typo can't silently change a condition.
pub fn parse_allowlist(spec: Option<&str>, all: &[String]) -> Result<Option<BTreeSet<String>>> {
    let Some(spec) = spec.map(str::trim).filter(|s| !s.is_empty()) else { return Ok(None) };
    let mut out = BTreeSet::new();
    for tok in spec.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()) {
        if tok.eq_ignore_ascii_case("all") {
            return Ok(None);
        }
        if let Some((_, tools)) = CONDITIONS.iter().find(|(c, _)| c.eq_ignore_ascii_case(tok)) {
            out.extend(tools.iter().map(|t| t.to_string()));
        } else if all.iter().any(|t| t == tok) {
            out.insert(tok.to_string());
        } else {
            bail!(
                "SIMBRIDGE_TOOLS: unknown tool or condition {tok:?} (conditions: C2, C3, C4; tools: {})",
                all.join(", ")
            );
        }
    }
    Ok(Some(out))
}

pub struct CallLog {
    file: Option<Mutex<File>>,
    start: Instant,
    seq: AtomicU64,
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl CallLog {
    /// Opens `SIMBRIDGE_LOG` for appending (if set) and writes a start line.
    pub fn from_env(tools: &[String]) -> Result<Self> {
        let file = match std::env::var_os("SIMBRIDGE_LOG") {
            Some(p) => {
                let f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&p)
                    .with_context(|| format!("opening SIMBRIDGE_LOG {}", p.to_string_lossy()))?;
                Some(Mutex::new(f))
            }
            None => None,
        };
        let log = Self { file, start: Instant::now(), seq: AtomicU64::new(0) };
        log.write(json!({"event": "start", "t_unix_ms": unix_ms(), "pid": std::process::id(),
                         "version": env!("CARGO_PKG_VERSION"), "tools": tools}));
        Ok(log)
    }

    fn write(&self, v: Value) {
        if let Some(f) = &self.file {
            let mut f = f.lock().unwrap_or_else(|e| e.into_inner());
            // One write per line so concurrent readers never see half a record.
            let _ = f.write_all(format!("{v}\n").as_bytes());
            let _ = f.flush();
        }
    }

    pub fn enabled(&self) -> bool {
        self.file.is_some()
    }

    /// One line per tool call. `result_bytes` is the size of the text handed back
    /// to the agent, i.e. what this call added to its context.
    pub fn record(&self, tool: &str, args: &Value, res: &Result<CallToolResponse, rmcp::ErrorData>, wall_ms: f64) {
        if !self.enabled() {
            return;
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let mut line = json!({"event": "call", "seq": seq, "t_unix_ms": unix_ms(),
                              "t_wall_ms": (self.start.elapsed().as_secs_f64() * 10_000.0).round() / 10.0,
                              "tool": tool, "args": args, "wall_ms": wall_ms});
        match res {
            Ok(CallToolResponse::Complete(r)) => {
                let text: String = r
                    .content
                    .iter()
                    .filter_map(|b| b.as_text().map(|t| t.text.as_str()))
                    .collect::<Vec<_>>()
                    .join("");
                line["result_bytes"] = json!(text.len());
                let parsed: Option<Value> = serde_json::from_str(&text).ok();
                let ok = parsed.as_ref().and_then(|v| v["ok"].as_bool()).unwrap_or(r.is_error != Some(true));
                line["ok"] = json!(ok);
                line["t_virtual_s"] = parsed.as_ref().map(|v| v["virtual_time_s"].clone()).unwrap_or(Value::Null);
                if !ok {
                    let err = parsed.as_ref().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or(text);
                    line["error"] = json!(err.chars().take(300).collect::<String>());
                }
            }
            Ok(_) => {
                line["ok"] = json!(false);
                line["error"] = json!("non-complete tool response");
            }
            Err(e) => {
                // Includes calls to tools hidden by the allowlist.
                line["ok"] = json!(false);
                line["result_bytes"] = json!(0);
                line["error"] = json!(e.message.chars().take(300).collect::<String>());
            }
        }
        self.write(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<String> {
        CONDITIONS.last().unwrap().1.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn allowlist_parsing() {
        assert_eq!(parse_allowlist(None, &all()).unwrap(), None);
        assert_eq!(parse_allowlist(Some("all"), &all()).unwrap(), None);
        let c2 = parse_allowlist(Some("C2"), &all()).unwrap().unwrap();
        assert_eq!(c2.len(), 5);
        assert!(!c2.contains("regs"));
        let mixed = parse_allowlist(Some("c2, svd"), &all()).unwrap().unwrap();
        assert!(mixed.contains("svd") && mixed.contains("load") && !mixed.contains("regs"));
        assert!(parse_allowlist(Some("C3 regz"), &all()).is_err());
        assert_eq!(parse_allowlist(Some("C4"), &all()).unwrap().unwrap().len(), 11);
    }
}
