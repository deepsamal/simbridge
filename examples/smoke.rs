//! End-to-end test: spawn the simbridge binary and drive it over MCP stdio, exactly
//! as Claude Code would, on both boards.
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
    let exe = root.join("target/release/simbridge").with_extension(std::env::consts::EXE_EXTENSION);
    let client = ().serve(TokioChildProcess::new(tokio::process::Command::new(&exe))?).await?;
    let tools: Vec<String> = client.list_all_tools().await?.into_iter().map(|t| t.name.to_string()).collect();
    println!("tools: {tools:?}");

    let boards = [
        ("stm32f4", "usart2", "firmware/stm32f4-hello/target/thumbv7em-none-eabihf/release/stm32f4-hello"),
        ("fe310", "uart0", "firmware/fe310-hello/target/riscv32imac-unknown-none-elf/release/fe310-hello"),
    ];
    for (platform, uart, elf) in boards {
        println!("=== {platform}");
        let elf = root.join(elf);
        call(&client, "load", json!({"elf": elf, "platform": platform})).await?;
        call(&client, "peripheral_trace", json!({"peripheral": uart})).await?;
        call(&client, "run_for", json!({"virtual_ms": 50})).await?;
        let s = call(&client, "serial", json!({})).await?;
        ensure!(s["text"].as_str().unwrap_or("").contains("hello from"), "no banner");
        let t = call(&client, "peripheral_trace", json!({"peripheral": uart, "limit": 2})).await?;
        ensure!(t["total"].as_u64().unwrap_or(0) > 0, "empty trace");
        call(&client, "regs", json!({"peripheral": uart, "count": 4})).await?;
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
        call(&client, "run_for", json!({"virtual_ms": 10})).await?;
        call(&client, "serial", json!({})).await?;
    }
    client.cancel().await?;
    println!("MCP OK");
    Ok(())
}
