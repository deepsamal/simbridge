//! simbridge: MCP server (stdio) exposing a headless Renode simulator to coding agents.
//!
//! Every tool returns a small JSON object with `ok`, `virtual_time_s` (machine
//! virtual time after the call) and `wall_ms` (host time the call took).

mod calllog;
mod presets;
mod renode;
mod svd;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use renode::{ms, Renode};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct LoadArgs {
    /// Path to the firmware ELF.
    elf: String,
    /// Preset (stm32f4, fe310/hifive1, nrf52840, stm32f103, stm32f072, stm32l552,
    /// k210) or a path to a .repl file. Default stm32f4.
    platform: Option<String>,
    /// Console UART, e.g. `sysbus.usart2`. Required for a custom .repl.
    uart: Option<String>,
    /// CPU to report the PC of (default from the preset, else `sysbus.cpu`).
    cpu: Option<String>,
    /// CMSIS-SVD for register decoding: a path (absolute, or relative to the SVD
    /// directory), a preset name, or "none". Default: the platform preset's SVD.
    svd: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RunForArgs {
    /// Virtual milliseconds to advance.
    virtual_ms: f64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RunUntilArgs {
    /// Stop when UART output (since the last serial() call) matches this regex.
    uart_regex: Option<String>,
    /// Stop when the 32-bit word here (number, "0x..." or ELF symbol) equals mem_equals.
    mem_addr: Option<Value>,
    /// Value to compare against (number or "0x..." string).
    mem_equals: Option<Value>,
    /// Give up after this much virtual time (default 1000 ms).
    timeout_ms: Option<f64>,
    /// Check conditions every this many virtual ms (default 1); the stop-time resolution.
    step_ms: Option<f64>,
}

fn value_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SerialArgs {
    /// Keep at most this many trailing bytes (default 4096).
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RegsArgs {
    /// Peripheral name as in the platform, e.g. `usart2` or `uart0`.
    peripheral: String,
    /// Number of 32-bit words to read (default 8, max 64).
    count: Option<u32>,
    /// Byte offset into the peripheral window (default 0).
    offset: Option<u64>,
    /// Read one register by its SVD name (e.g. "SR"), instead of offset/count.
    register: Option<String>,
    /// Include register and field descriptions from the SVD.
    verbose: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SvdArgs {
    /// SVD to query: path, preset name (e.g. `nrf52840`) or omitted for the
    /// loaded machine's SVD.
    svd: Option<String>,
    /// Peripheral name as in the SVD (e.g. "USART2"). Omit to list peripherals.
    peripheral: Option<String>,
    /// Register name within the peripheral (e.g. "SR"). Omit to list registers.
    register: Option<String>,
    /// Decode this raw value (number or "0x...") against `register`.
    value: Option<Value>,
    /// Case-insensitive regex searched over peripheral/register/field names and
    /// descriptions, e.g. "TXE" or "baud". Ignores peripheral/register.
    query: Option<String>,
    /// Max search matches (default 40).
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MemArgs {
    /// Address (number or "0x..." string) or ELF symbol name, e.g. "TICKS".
    addr: Value,
    /// Bytes to read (default 64, max 1024).
    length: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TraceArgs {
    /// Peripheral name, e.g. `usart2`.
    peripheral: String,
    /// Return at most this many most recent accesses (default 50).
    limit: Option<usize>,
    /// Turn tracing off after this read.
    disable: Option<bool>,
    /// Decode each access value into SVD register fields.
    decode: Option<bool>,
    /// Keep only reads ("R") or writes ("W").
    op: Option<String>,
    /// Keep only accesses at this register offset (number or "0x..." string).
    offset: Option<Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MonitorArgs {
    /// One Renode monitor command, e.g. `sysbus.cpu ExecutedInstructions`.
    command: String,
}

/// Tools that exist only for the benchmark grader (`SIMBRIDGE_MODE=grader`).
/// They are removed in agent mode, so no allowlist can expose them to an agent.
const GRADER_ONLY: &[&str] = &["monitor"];

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RestoreArgs {
    /// Snapshot id returned by snapshot().
    id: String,
}

type State = Arc<Mutex<Option<Renode>>>;

#[derive(Clone)]
struct SimBridge {
    state: State,
    tool_router: ToolRouter<Self>,
    log: Arc<calllog::CallLog>,
}

impl SimBridge {
    /// Builds the router, drops tools not in `SIMBRIDGE_TOOLS` (they are neither
    /// listed nor callable), and opens `SIMBRIDGE_LOG`.
    fn new(state: State) -> anyhow::Result<Self> {
        let mut tool_router = Self::tool_router();
        let mode = std::env::var("SIMBRIDGE_MODE").unwrap_or_default();
        let grader = match mode.trim().to_ascii_lowercase().as_str() {
            "" | "agent" => false,
            "grader" => true,
            other => anyhow::bail!("SIMBRIDGE_MODE must be agent (default) or grader, not {other:?}"),
        };
        if !grader {
            for t in GRADER_ONLY {
                tool_router.remove_route(t);
            }
        }
        let all: Vec<String> = tool_router.list_all().iter().map(|t| t.name.to_string()).collect();
        let allow = calllog::parse_allowlist(std::env::var("SIMBRIDGE_TOOLS").ok().as_deref(), &all)?;
        if let Some(allow) = &allow {
            for t in all.iter().filter(|t| !allow.contains(*t)) {
                tool_router.remove_route(t);
            }
        }
        let exposed: Vec<String> = tool_router.list_all().iter().map(|t| t.name.to_string()).collect();
        let log = Arc::new(calllog::CallLog::from_env(&exposed, if grader { "grader" } else { "agent" })?);
        Ok(Self { state, tool_router, log })
    }

    /// Run a Renode operation on a blocking thread and wrap it in the common envelope.
    async fn call<F>(&self, f: F) -> String
    where
        F: FnOnce(&mut Renode) -> anyhow::Result<Value> + Send + 'static,
    {
        let state = self.state.clone();
        let out = tokio::task::spawn_blocking(move || {
            let t = Instant::now();
            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                let workdir = std::env::var_os("SIMBRIDGE_WORKDIR").map(PathBuf::from);
                match Renode::new(workdir) {
                    Ok(r) => *guard = Some(r),
                    Err(e) => return json!({"ok": false, "error": e.to_string(), "wall_ms": ms(t)}),
                }
            }
            let r = guard.as_mut().unwrap();
            let mut out = match f(r) {
                Ok(Value::Object(mut m)) => {
                    m.insert("ok".into(), json!(true));
                    Value::Object(m)
                }
                Ok(v) => json!({"ok": true, "result": v}),
                Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
            };
            out["virtual_time_s"] = if r.is_loaded() {
                json!(r.virtual_time_s().ok().flatten())
            } else {
                json!(0.0)
            };
            out["wall_ms"] = json!(ms(t));
            out
        })
        .await
        .unwrap_or_else(|e| json!({"ok": false, "error": format!("worker panicked: {e}")}));
        out.to_string()
    }
}

#[tool_router]
impl SimBridge {
    #[tool(description = "Create a fresh machine and load an ELF. platform: a preset \
        (stm32f4 [usart2], fe310/hifive1 [uart0], nrf52840 [uart0], stm32f103 [usart2], \
        stm32f072 [usart2], stm32l552 [usart2], k210 [uart]; each with its SVD) or a path to \
        a .repl file (then `uart` is required). svd overrides the SVD (path, preset or \
        \"none\"). Starts paused at virtual time 0; use run_for to advance.")]
    async fn load(&self, Parameters(a): Parameters<LoadArgs>) -> String {
        self.call(move |r| {
            r.load(
                a.elf.as_ref(),
                a.platform.as_deref().unwrap_or("stm32f4"),
                a.uart.as_deref(),
                a.cpu.as_deref(),
                a.svd.as_deref(),
            )
        })
        .await
    }

    #[tool(description = "Rebuild the machine from scratch and reload the same ELF \
        (virtual time -> 0). Peripheral traces stay enabled; serial buffer is cleared.")]
    async fn reset(&self) -> String {
        self.call(|r| r.reset()).await
    }

    #[tool(description = "Advance the simulation by exactly `virtual_ms` of virtual time \
        (deterministic), then pause. Returns the program counter.")]
    async fn run_for(&self, Parameters(a): Parameters<RunForArgs>) -> String {
        self.call(move |r| r.run_for(a.virtual_ms)).await
    }

    #[tool(description = "Run until a condition holds, checking every step_ms (default 1) of \
        virtual time, up to timeout_ms (default 1000). Conditions: uart_regex matches UART \
        output, and/or the 32-bit word at mem_addr (address or ELF symbol) equals mem_equals. \
        Returns hit, stop reason, elapsed_virtual_ms and the UART text seen (consumed, so a \
        later serial() won't repeat it).")]
    async fn run_until(&self, Parameters(a): Parameters<RunUntilArgs>) -> String {
        self.call(move |r| {
            let want = match &a.mem_equals {
                None => None,
                Some(v) => {
                    let s = value_str(v);
                    let s = s.trim();
                    Some(match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                        Some(h) => u64::from_str_radix(h, 16)?,
                        None => s.parse::<u64>()?,
                    })
                }
            };
            r.run_until(
                a.uart_regex.as_deref(),
                a.mem_addr.as_ref().map(value_str).as_deref(),
                want,
                a.timeout_ms.unwrap_or(1000.0),
                a.step_ms.unwrap_or(1.0),
            )
        })
        .await
    }

    #[tool(description = "UART console output captured since the previous serial() call \
        (tail-truncated to max_bytes).")]
    async fn serial(&self, Parameters(a): Parameters<SerialArgs>) -> String {
        self.call(move |r| r.serial(a.max_bytes.unwrap_or(4096))).await
    }

    #[tool(description = "Read 32-bit words from a peripheral's memory-mapped window, e.g. \
        peripheral=\"usart2\". With an SVD loaded each word is decoded into registers and \
        fields (value, bit range, enum name); register=\"SR\" reads one register by name; \
        verbose adds descriptions. Reads go through the model, so registers with read side \
        effects (FIFOs, clear-on-read flags; flagged as read_side_effect) are affected.")]
    async fn regs(&self, Parameters(a): Parameters<RegsArgs>) -> String {
        self.call(move |r| {
            r.regs(
                &a.peripheral,
                a.count.unwrap_or(8),
                a.offset.unwrap_or(0),
                a.register.as_deref(),
                a.verbose.unwrap_or(false),
            )
        })
        .await
    }

    #[tool(description = "Look things up in a CMSIS-SVD without touching the simulator. \
        No args: list peripherals (name, base, description). peripheral: its registers \
        (offset, size, access, reset). peripheral+register: every field with bit range, \
        access, description and enumerated values. Add value to decode a raw number. \
        query: regex search over names/descriptions (find which register holds a flag). \
        Uses the loaded machine's SVD unless svd (path or preset) is given.")]
    async fn svd(&self, Parameters(a): Parameters<SvdArgs>) -> String {
        let state = self.state.clone();
        let t = Instant::now();
        let out = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
            let path = match a.svd.as_deref() {
                Some(s) => svd::resolve(Some(s), "")?
                    .ok_or_else(|| anyhow::anyhow!("svd=\"none\" has nothing to query"))?,
                None => state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .and_then(|r| r.loaded_svd_path())
                    .ok_or_else(|| {
                        anyhow::anyhow!("no machine with an SVD loaded; pass svd (path or preset)")
                    })?,
            };
            let dev = svd::load(&path)?;
            if let Some(q) = &a.query {
                return dev.search(q, a.limit.unwrap_or(40));
            }
            let Some(pn) = &a.peripheral else { return Ok(dev.summary()) };
            let p = dev.require(pn)?;
            let Some(rn) = &a.register else {
                let mut v = p.summary();
                v["registers"] = p.registers.iter().map(|r| r.summary()).collect();
                return Ok(v);
            };
            let reg = p.register(rn).ok_or_else(|| {
                anyhow::anyhow!("{} has no register {rn:?}; call svd(peripheral={:?})", p.name, p.name)
            })?;
            let mut v = reg.describe();
            v["peripheral"] = json!(p.name);
            v["address"] = json!(format!("0x{:08X}", p.base + reg.offset));
            if let Some(val) = &a.value {
                let s = value_str(val);
                let s = s.trim();
                let n = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                    Some(h) => u64::from_str_radix(h, 16)?,
                    None => s.parse::<u64>()?,
                };
                v["decoded"] = reg.decode(n, false);
            }
            Ok(v)
        })
        .await;
        let mut v = match out {
            Ok(Ok(Value::Object(mut m))) => {
                m.insert("ok".into(), json!(true));
                Value::Object(m)
            }
            Ok(Ok(v)) => json!({"ok": true, "result": v}),
            Ok(Err(e)) => json!({"ok": false, "error": format!("{e:#}")}),
            Err(e) => json!({"ok": false, "error": format!("worker panicked: {e}")}),
        };
        v["wall_ms"] = json!(renode::ms(t));
        v.to_string()
    }

    #[tool(description = "Read up to 1024 bytes of memory. `addr` is a number, a \"0x...\" \
        string or an ELF symbol name (e.g. \"TICKS\"). Returns hex.")]
    async fn mem(&self, Parameters(a): Parameters<MemArgs>) -> String {
        let addr = match &a.addr {
            Value::String(s) => s.clone(),
            v => v.to_string(),
        };
        self.call(move |r| r.mem(&addr, a.length.unwrap_or(64))).await
    }

    #[tool(description = "Bus accesses to a peripheral (via `sysbus LogPeripheralAccess`) \
        logged since the previous call: pc, op (R/W), offset, register name, value. The first \
        call enables tracing and returns nothing; run_for, then call again. `total` counts \
        every matching access; only the last `limit` are returned. Filter with op/offset \
        (e.g. op=\"W\", offset=\"0x4\") rather than raising limit.")]
    async fn peripheral_trace(&self, Parameters(a): Parameters<TraceArgs>) -> String {
        self.call(move |r| {
            let filter = renode::TraceFilter::parse(a.op.as_deref(), a.offset.as_ref())?;
            r.peripheral_trace(
                &a.peripheral,
                a.limit.unwrap_or(50),
                a.disable.unwrap_or(false),
                a.decode.unwrap_or(false),
                &filter,
            )
        })
        .await
    }

    #[tool(description = "Grader only: run one raw Renode monitor command and return its \
        text output (e.g. `sysbus.cpu ExecutedInstructions`). Errors come back in the output.")]
    async fn monitor(&self, Parameters(a): Parameters<MonitorArgs>) -> String {
        self.call(move |r| r.monitor(&a.command)).await
    }

    #[tool(description = "Save full emulation state (Renode `Save`). Returns an id for \
        restore() and how long the save took.")]
    async fn snapshot(&self) -> String {
        self.call(|r| r.snapshot()).await
    }

    #[tool(description = "Restore a snapshot taken with snapshot() (Renode `Load`): virtual \
        time, CPU and memory state. Returns how long the load took.")]
    async fn restore(&self, Parameters(a): Parameters<RestoreArgs>) -> String {
        self.call(move |r| r.restore(&a.id)).await
    }
}

#[tool_handler(
    router = self.tool_router,
    name = "simbridge",
    version = "0.1.0",
    instructions = "Drive a headless Renode simulator: load an ELF, run_for virtual ms, \
        read serial/regs/mem, trace peripheral accesses, snapshot/restore. Board presets \
        come with a CMSIS-SVD, so regs() returns named fields; use svd() to look up \
        registers, fields and enum values instead of guessing bit layouts."
)]
impl ServerHandler for SimBridge {
    /// Same as the generated handler, plus one log line per call.
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
        let t = Instant::now();
        let name = request.name.to_string();
        let args = Value::Object(request.arguments.clone().unwrap_or_default());
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        let res = self.tool_router.call(tcc).await;
        self.log.record(&name, &args, &res, ms(t));
        res
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let state: State = Arc::new(Mutex::new(None));
    let service = SimBridge::new(state.clone())?.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    // Client went away: shut Renode down.
    if let Some(mut r) = state.lock().unwrap_or_else(|e| e.into_inner()).take() {
        r.stop();
    }
    Ok(())
}
