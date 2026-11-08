# simbridge (prototype)

An MCP server, written in Rust, that gives a coding agent a headless
[Renode](https://renode.io) simulator to load firmware into, run it for a
fixed amount of virtual time, and inspect the results: UART output,
registers, memory and bus traces, plus snapshots it can restore.

It's one native binary on the official Rust MCP SDK
([`rmcp`](https://crates.io/crates/rmcp)) over stdio. It starts
`renode --disable-xwt --plain --port N` on the first tool call and drives it
through the monitor socket. It uses no pyrenode3 or .NET interop, and it
stops Renode when the client disconnects.

## Tools

Every result is JSON with `ok`, `virtual_time_s` (the machine's elapsed virtual
time after the call) and `wall_ms` (the host time the call took).

| tool | what it does |
|---|---|
| `load(elf, platform="stm32f4", uart?)` | Creates a fresh machine and loads the ELF. Presets: `stm32f4` (`platforms/cpus/stm32f4.repl`, console `usart2`) and `fe310`/`hifive1` (`platforms/cpus/sifive-fe310.repl`, console `uart0`). Pass a `.repl` path and a `uart` to use any other board. |
| `reset()` | Rebuilds the machine and reloads the same ELF, so virtual time goes back to 0. Traces stay enabled. |
| `run_for(virtual_ms)` | Advances exactly that much virtual time, then pauses. Returns the PC. |
| `serial(max_bytes=4096)` | Returns UART output since the previous call. |
| `regs(peripheral, count=8, offset=0)` | Reads 32-bit words from a peripheral's bus window. These are model reads, so registers with read side effects are affected. |
| `mem(addr, length=64)` | Reads up to 1 KiB. `addr` can be a number, a `"0x..."` string or an ELF symbol such as `"TICKS"`. |
| `peripheral_trace(peripheral, limit=50, disable=false)` | The first call turns on `LogPeripheralAccess`. Later calls return the accesses since the previous call: PC, R/W, offset, register name and value. |
| `snapshot()` / `restore(id)` | Renode `Save` / `Load` of the full state. Each reports how long it took. |

## Requirements

- Renode 1.17 (tested on Windows at `C:\Program Files\Renode`). Set
  `SIMBRIDGE_RENODE` if it isn't on PATH or in a default location.
  `SIMBRIDGE_WORKDIR` sets where logs and snapshots go (the default is a temp dir).
- Rust stable (tested on 1.94).
- Test firmware: `rustup target add thumbv7em-none-eabihf riscv32imac-unknown-none-elf`.

## Build and test

```bash
cargo build --release
(cd firmware/stm32f4-hello && cargo build --release)
(cd firmware/fe310-hello && cargo build --release)
cargo run --release --example smoke
```

`examples/smoke.rs` spawns the server binary and drives every tool over real
MCP stdio on both boards. It checks the UART banner, that the trace isn't
empty, and that `restore` rewinds the `TICKS` counter in RAM.

## Add to Claude Code

```bash
claude mcp add simbridge -- "C:/Users/Deep Samal/agentic-embedded-rust/simbridge/target/release/simbridge.exe"
```

Then ask Claude something like: "load firmware/stm32f4-hello's release ELF on
stm32f4, run 50 ms, show serial and the usart2 trace."

## Measured (Windows, Renode 1.17.0)

| | STM32F4 | FE310 |
|---|---|---|
| snapshot (Save) | ~0.66 s, 2.8 MB | ~0.10 s, 0.9 MB |
| restore (Load + UART reattach) | ~1.2 s | ~0.3 s |
| run_for 50 ms virtual | ~60–110 ms wall | ~60 ms wall |
| Renode cold start (first call) | ~2.5 s | |

The STM32F4 snapshot is bigger because that platform maps more memory: 2 MiB
of flash plus a 256 MiB FSMC window.

## Notes and limits

- On Windows, Renode keeps UART file backends locked, so UART output is read
  from a `CreateServerSocketTerminal`. Terminals aren't saved in snapshots, so
  `restore` reattaches one.
- `mach clear` only deselects the current machine; `load` uses `Clear`.
- In the trace log, Renode tags lines `usart2:` when there is one machine and
  `machine/usart2:` when there are several. The parser accepts both.
- SVD: Renode has `sysbus ApplySVD @file.svd`, which adds register names to
  trace lines for regions without a model. The built-in STM32 models already
  name their registers. Decoding fields in `regs` from an SVD is the obvious
  next step.
- `python/` holds the first Python version of the same server. Its
  `python/tests/smoke_core.py` runs on Python 3.8 without the MCP SDK. It's
  kept for reference; the Rust binary is the real server.
