//! End-to-end test: spawn the simbridge binary and drive it over MCP stdio, exactly
//! as Claude Code would, on every preset board.
//!
//!     cargo build --release && cargo run --release --example smoke

use std::path::PathBuf;

use anyhow::{ensure, Context, Result};
use rmcp::{
    model::CallToolRequestParams,
    service::{RoleClient, RunningService},
    transport::TokioChildProcess,
    ServiceExt,
};
use serde_json::{json, Value};

async fn call(c: &RunningService<RoleClient, ()>, name: &'static str, args: Value) -> Result<Value> {
    let p = CallToolRequestParams::new(name).with_arguments(args.as_object().cloned().unwrap_or_default());
    let res = c.call_tool(p).await?;
    let text = res.content.first().and_then(|b| b.as_text()).context("no text content")?.text.clone();
    let v: Value = serde_json::from_str(&text)?;
    let s = v.to_string();
    println!("{name:<17} {}", &s[..s.len().min(260)]);
    ensure!(v["ok"] == json!(true), "{name} failed: {v}");
    Ok(v)
}

#[tokio::main]
async fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // `cargo run --example smoke` builds only this example, never the server
    // binary it drives, so build that first or we'd test a stale simbridge.
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = std::process::Command::new(cargo)
        .args(["build", "--release", "--bin", "simbridge"])
        .current_dir(&root)
        .status()?;
    ensure!(status.success(), "building simbridge failed");
    let exe = root.join("target/release/simbridge").with_extension(std::env::consts::EXE_EXTENSION);
    // The main pass needs every tool and no log, whatever the caller's environment says.
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.env_remove("SIMBRIDGE_TOOLS").env_remove("SIMBRIDGE_LOG").env_remove("SIMBRIDGE_MODE");
    let client = ().serve(TokioChildProcess::new(cmd)?).await?;
    let tools: Vec<String> = client.list_all_tools().await?.into_iter().map(|t| t.name.to_string()).collect();
    println!("tools: {tools:?}");

    // (preset, console UART, ELF, SVD peripheral, a status register to decode)
    let boards = [
        ("stm32f4", "usart2", "firmware/stm32f4-hello/target/thumbv7em-none-eabihf/release/stm32f4-hello",
         "USART2", "SR"),
        ("fe310", "uart0", "firmware/fe310-hello/target/riscv32imac-unknown-none-elf/release/fe310-hello",
         "UART0", "txdata"),
        ("nrf52840", "uart0", "firmware/nrf52840-hello/target/thumbv7em-none-eabihf/release/nrf52840-hello",
         "UARTE0", "ENABLE"),
        ("stm32f103", "usart2", "firmware/stm32f103-hello/target/thumbv7m-none-eabi/release/stm32f103-hello",
         "USART2", "SR"),
        ("stm32f072", "usart2", "firmware/stm32f072-hello/target/thumbv6m-none-eabi/release/stm32f072-hello",
         "USART2", "ISR"),
        ("stm32l552", "usart2", "firmware/stm32l552-hello/target/thumbv8m.main-none-eabihf/release/stm32l552-hello",
         "USART2", "ISR"),
        ("k210", "uart", "firmware/k210-hello/target/riscv64imac-unknown-none-elf/release/k210-hello",
         "UARTHS", "txdata"),
    ];
    // `cargo run --example smoke -- nrf52840 k210` runs just those boards.
    let only: Vec<String> = std::env::args().skip(1).collect();
    for (platform, uart, elf, svd_periph, status_reg) in boards {
        if !only.is_empty() && !only.iter().any(|o| o == platform) {
            continue;
        }
        println!("=== {platform}");
        let elf = root.join(elf);
        call(&client, "load", json!({"elf": elf, "platform": platform})).await?;
        call(&client, "peripheral_trace", json!({"peripheral": uart})).await?;
        call(&client, "run_for", json!({"virtual_ms": 50})).await?;
        let s = call(&client, "serial", json!({})).await?;
        ensure!(s["text"].as_str().unwrap_or("").contains("hello from"), "no banner");
        let t = call(&client, "peripheral_trace", json!({"peripheral": uart, "limit": 2})).await?;
        // Every access carries a virtual time within the 50 ms just run.
        ensure!(
            t["accesses"].as_array().is_some_and(|a| a.iter().all(|x| x["t"].as_f64().is_some_and(|v| v <= 0.0501))),
            "trace accesses lack virtual timestamps: {t}"
        );
        ensure!(t["total"].as_u64().unwrap_or(0) > 0, "empty trace");
        let r = call(&client, "regs", json!({"peripheral": uart, "count": 4})).await?;
        ensure!(r["svd_peripheral"] == json!(svd_periph), "regs not matched to SVD: {r}");
        let r = call(&client, "regs", json!({"peripheral": uart, "register": status_reg})).await?;
        ensure!(r["words"][0]["registers"][0]["fields"].is_array(), "no decoded fields: {r}");
        let s = call(&client, "svd", json!({"peripheral": svd_periph, "register": status_reg})).await?;
        ensure!(s["fields"].as_array().is_some_and(|f| !f.is_empty()), "svd lookup empty");
        call(&client, "svd", json!({"query": status_reg, "limit": 3})).await?;
        let t = call(&client, "peripheral_trace", json!({"peripheral": uart, "limit": 2, "decode": true})).await?;
        ensure!(t["total"].as_u64() == Some(0) || t["accesses"][0]["decoded"].is_array(), "trace not decoded");
        let before = call(&client, "mem", json!({"addr": "TICKS", "length": 4})).await?;
        let snap = call(&client, "snapshot", json!({})).await?;
        call(&client, "run_for", json!({"virtual_ms": 50})).await?;
        let moved = call(&client, "mem", json!({"addr": "TICKS", "length": 4})).await?;
        ensure!(moved["hex"] != before["hex"], "TICKS did not advance");
        call(&client, "serial", json!({})).await?;
        call(&client, "restore", json!({"id": snap["id"]})).await?;
        let after = call(&client, "mem", json!({"addr": "TICKS", "length": 4})).await?;
        ensure!(after["hex"] == before["hex"], "restore did not rewind TICKS");
        call(&client, "run_for", json!({"virtual_ms": 20})).await?;
        call(&client, "serial", json!({})).await?;
        call(&client, "reset", json!({})).await?;
        let u = call(&client, "run_until", json!({"uart_regex": r"tick 5\r", "timeout_ms": 500})).await?;
        ensure!(u["hit"] == json!(true) && u["stop"]["reason"] == json!("uart"), "run_until uart");
        let m = call(&client, "run_until", json!({"mem_addr": "TICKS", "mem_equals": 20, "timeout_ms": 500})).await?;
        ensure!(m["hit"] == json!(true) && m["stop"]["reason"] == json!("mem"), "run_until mem");
        let miss = call(&client, "run_until", json!({"uart_regex": "never", "timeout_ms": 5})).await?;
        ensure!(miss["hit"] == json!(false), "run_until timeout");
        call(&client, "reset", json!({})).await?;
        call(&client, "run_for", json!({"virtual_ms": 10})).await?;
        call(&client, "serial", json!({})).await?;
    }
    client.cancel().await?;

    // Feedback condition C2 + call log: a second server exposing only the console tools.
    println!("=== C2 allowlist + call log");
    let log_path = std::env::temp_dir().join(format!("simbridge-smoke-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&log_path);
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.env("SIMBRIDGE_TOOLS", "C2").env("SIMBRIDGE_LOG", &log_path).env_remove("SIMBRIDGE_MODE");
    let c2 = ().serve(TokioChildProcess::new(cmd)?).await?;
    let mut names: Vec<String> = c2.list_all_tools().await?.into_iter().map(|t| t.name.to_string()).collect();
    names.sort();
    ensure!(names == ["load", "reset", "run_for", "run_until", "serial"], "C2 exposes {names:?}");
    let regs = CallToolRequestParams::new("regs")
        .with_arguments(json!({"peripheral": "usart2"}).as_object().cloned().unwrap_or_default());
    ensure!(c2.call_tool(regs).await.is_err(), "regs was callable under C2");
    let elf = root.join(boards[0].2); // stm32f4
    call(&c2, "load", json!({"elf": elf, "platform": "stm32f4"})).await?;
    call(&c2, "run_for", json!({"virtual_ms": 50})).await?;
    call(&c2, "serial", json!({})).await?;
    c2.cancel().await?;
    let lines: Vec<Value> = std::fs::read_to_string(&log_path)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    println!("call log: {} lines, last: {}", lines.len(), lines.last().map(|l| l.to_string()).unwrap_or_default());
    ensure!(lines.len() == 5, "expected start + 4 calls in the log, got {}", lines.len());
    ensure!(lines[0]["event"] == json!("start"), "first log line is not the start event");
    ensure!(lines[1]["tool"] == json!("regs") && lines[1]["ok"] == json!(false), "hidden call not logged as failed");
    ensure!(lines[3]["tool"] == json!("run_for") && lines[3]["t_virtual_s"] == json!(0.05), "run_for virtual time not logged");
    ensure!(lines[4]["result_bytes"].as_u64().unwrap_or(0) > 0, "serial result size not logged");
    let _ = std::fs::remove_file(&log_path);

    // Grader mode: the grader-only `monitor` tool appears and runs raw monitor commands.
    println!("=== grader mode");
    ensure!(!tools.iter().any(|t| t == "monitor"), "monitor exposed in agent mode");
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.env("SIMBRIDGE_MODE", "grader").env_remove("SIMBRIDGE_TOOLS").env_remove("SIMBRIDGE_LOG");
    let gr = ().serve(TokioChildProcess::new(cmd)?).await?;
    let gtools: Vec<String> = gr.list_all_tools().await?.into_iter().map(|t| t.name.to_string()).collect();
    ensure!(gtools.iter().any(|t| t == "monitor"), "grader mode lacks monitor: {gtools:?}");
    call(&gr, "load", json!({"elf": elf, "platform": "stm32f4"})).await?;
    // Trace read immediately after a very short run: Renode writes its log
    // asynchronously, so this fails if peripheral_trace doesn't wait for it.
    call(&gr, "peripheral_trace", json!({"peripheral": "usart2"})).await?;
    call(&gr, "run_for", json!({"virtual_ms": 1})).await?;
    let t = call(&gr, "peripheral_trace", json!({"peripheral": "usart2", "limit": 1000})).await?;
    ensure!(t["total"].as_u64().unwrap_or(0) > 0 && t.get("warning").is_none(), "trace missed fresh accesses: {t}");
    let times: Vec<f64> = t["accesses"].as_array().map(|a| a.iter().filter_map(|x| x["t"].as_f64()).collect()).unwrap_or_default();
    ensure!(!times.is_empty() && times.windows(2).all(|w| w[0] <= w[1]) && times.last() <= Some(&0.0011),
            "timestamps not ordered within the 1 ms run: {times:?}");
    let s = call(&gr, "serial", json!({})).await?;
    ensure!(s["t_from"].as_f64() == Some(0.0) && s["t_to"].as_f64().is_some_and(|v| v > 0.0), "serial window: {s}");
    call(&gr, "run_for", json!({"virtual_ms": 10})).await?;
    let pc = call(&gr, "monitor", json!({"command": "sysbus.cpu PC"})).await?;
    ensure!(pc["output"].as_str().is_some_and(|o| o.contains("0x")), "monitor PC: {pc}");
    gr.cancel().await?;
    println!("MCP OK");
    Ok(())
}
