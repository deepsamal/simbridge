//! simbridge: MCP server (stdio) exposing a headless Renode simulator to coding agents.
//!
//! Every tool returns a small JSON object with `ok`, `virtual_time_s` (machine
//! virtual time after the call) and `wall_ms` (host time the call took).

mod renode;

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
    /// Preset (`stm32f4`, `fe310`/`hifive1`) or a path to a .repl file. Default stm32f4.
    platform: Option<String>,
    /// Console UART, e.g. `sysbus.usart2`. Required for a custom .repl.
    uart: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RunForArgs {
    /// Virtual milliseconds to advance.
    virtual_ms: f64,
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
}

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
}

impl SimBridge {
    fn new(state: State) -> Self {
        Self { state, tool_router: Self::tool_router() }
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
    #[tool(description = "Create a fresh machine and load an ELF. platform: preset `stm32f4` \
        (platforms/cpus/stm32f4.repl, console sysbus.usart2) or `fe310`/`hifive1` \
        (platforms/cpus/sifive-fe310.repl, console sysbus.uart0), or a path to a .repl file \
        (then `uart` is required). Starts paused at virtual time 0; use run_for to advance.")]
    async fn load(&self, Parameters(a): Parameters<LoadArgs>) -> String {
        self.call(move |r| {
            r.load(a.elf.as_ref(), a.platform.as_deref().unwrap_or("stm32f4"), a.uart.as_deref())
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

    #[tool(description = "UART console output captured since the previous serial() call \
        (tail-truncated to max_bytes).")]
    async fn serial(&self, Parameters(a): Parameters<SerialArgs>) -> String {
        self.call(move |r| r.serial(a.max_bytes.unwrap_or(4096))).await
    }

    #[tool(description = "Read 32-bit words from a peripheral's memory-mapped window, e.g. \
        peripheral=\"usart2\". Reads go through the model, so registers with read side \
        effects (FIFOs, clear-on-read flags) are affected.")]
    async fn regs(&self, Parameters(a): Parameters<RegsArgs>) -> String {
        self.call(move |r| r.regs(&a.peripheral, a.count.unwrap_or(8), a.offset.unwrap_or(0)))
            .await
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
        call enables tracing and returns nothing; run_for, then call again.")]
    async fn peripheral_trace(&self, Parameters(a): Parameters<TraceArgs>) -> String {
        self.call(move |r| {
            r.peripheral_trace(&a.peripheral, a.limit.unwrap_or(50), a.disable.unwrap_or(false))
        })
        .await
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
        read serial/regs/mem, trace peripheral accesses, snapshot/restore."
)]
impl ServerHandler for SimBridge {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let state: State = Arc::new(Mutex::new(None));
    let service = SimBridge::new(state.clone()).serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    // Client went away: shut Renode down.
    if let Some(mut r) = state.lock().unwrap_or_else(|e| e.into_inner()).take() {
        r.stop();
    }
    Ok(())
}
