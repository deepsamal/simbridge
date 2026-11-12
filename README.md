# simbridge

**Firmware-in-the-loop development for coding agents.** simbridge runs the agent's
real compiled firmware, the same unmodified ELF that would be flashed to a
board, on a deterministic simulated microcontroller, and returns what it did:
UART output, registers decoded into named fields from the chip's CMSIS-SVD,
the firmware's actual peripheral reads and writes, and memory. The agent can
run for an exact amount of virtual time, stop on a condition, and snapshot and
restore the whole machine.

The simulator is [Renode](https://renode.io), driven headless; the agent talks
to simbridge over the Model Context Protocol (MCP). It's one native Rust binary,
built on the official Rust MCP SDK ([`rmcp`](https://crates.io/crates/rmcp)),
and works with Claude Code, Claude Desktop, or any other MCP client.

The tools are one half of the project. The other half, in progress, is the
guidance that makes agents good at embedded work: skills and instructions for
tasks such as looking up registers in an SVD instead of guessing bit positions,
and a benchmark that measures which of them actually help.

### Where firmware-in-the-loop fits

| stage | what runs | what it's simulated against | catches |
|---|---|---|---|
| Software-in-the-loop | application logic compiled for the host | stubs and mocks | logic errors |
| **Firmware-in-the-loop (simbridge)** | the real target binary, on the target's instruction set | modeled CPU, memory map and peripherals | register-level, interrupt, boot and porting errors |
| Hardware-in-the-loop | the real binary on a real board | simulated environment around the board | timing, power, analog and silicon-specific behaviour |

Firmware-in-the-loop sits before hardware-in-the-loop, not instead of it:
iterate quickly and deterministically here, then validate timing, power and
analog behaviour on real boards (see [Limits](#limits)).

## Why

An agent writing firmware can't see the hardware. Without the firmware in a
loop it can reach, every iteration goes through a person: the agent writes
code, someone flashes the board, and someone reports back what happened.
simbridge closes that loop so the agent can build, run, observe and fix on its
own:

- **Deterministic runs.** `run_for` and `run_until` advance virtual time
  exactly, so the same firmware gives the same result every time. An agent can
  tell "my change fixed it" apart from "it was flaky".
- **Observations, not guesses.** UART text, register fields by name, and the
  exact sequence of register accesses the code made, with the PC of each one.
  "It prints nothing" becomes "it spins at `0x4a4` polling `EVENTS_TXDRDY`,
  which never gets set".
- **The chip's facts from the vendor file.** The `svd` tool answers "which bit
  is TXE on this part?" or "where is `USART2EN`?" from the SVD instead of from
  the model's memory. A Cortex-M SVD can be 2–4 MB, so the tool returns only
  the part asked for.
- **Cheap experiments.** Snapshot before a risky step, try several fixes and
  restore between them. Bisecting a bug takes seconds, not reflashes.
- **No hardware needed,** so the same loop runs in CI or across parallel agents.

simbridge isn't tied to Rust. It loads any ELF, whether it was built from C,
C++, Zig or Rust, on Zephyr, FreeRTOS or bare metal. It fits embedded Rust
especially well, though: Rust PACs are generated from the same SVDs by
svd2rust, so the agent's code and simbridge's register decoding share one
source of truth.

## What a session looks like

```text
load(elf="firmware/nrf52840-hello/target/thumbv7em-none-eabihf/release/nrf52840-hello",
     platform="nrf52840")
run_for(virtual_ms=50)                 -> {"pc": "0x61a", "virtual_time_s": 0.05}
serial()                               -> "hello from nrf52840 (Rust no_std)\r\ntick 1\r\n..."
regs(peripheral="uart0", register="ENABLE")
  -> UARTE0.ENABLE = 0x8, fields: [{"name": "ENABLE", "bits": "3:0", "value": 8, "is": "Enabled"}]
svd(svd="stm32l552", query="^USART2EN$")
  -> RCC.APB1ENR1.USART2EN, bit 17
peripheral_trace(peripheral="uart0", decode=true)
  -> [{"pc": "0x55A", "op": "W", "offset": "0x544", "reg": "TxDPointer", "value": "0x20000000", ...}, ...]
snapshot()  ...  restore(id="s1")      -> virtual time and RAM back where they were
```

## Quick start

### 1. Requirements

- [Renode](https://renode.io) 1.17. On Windows the installer puts it at
  `C:\Program Files\Renode\renode.exe`, which simbridge finds on its own.
  Otherwise put it on `PATH` or set `SIMBRIDGE_RENODE`.
- Rust stable (tested on 1.94).

### 2. Build

```bash
git clone https://github.com/deepsamal/simbridge.git
cd simbridge
cargo build --release
```

### 3. Get the SVDs (optional, for register decoding)

simbridge looks for a folder named `svd` next to the executable or in any of
its parent folders. A sibling of the repo works:

```bash
cd ..                      # the folder that contains simbridge/
mkdir svd && cd svd
# cmsis-svd-data is several GB; check out only the vendors the presets use
git clone --depth 1 --filter=blob:none --sparse https://github.com/cmsis-svd/cmsis-svd-data
git -C cmsis-svd-data sparse-checkout set data/STMicro data/Nordic data/Kendryte-Community
git clone --depth 1 https://github.com/riscv-rust/e310x
```

You can also set `SIMBRIDGE_SVD_DIR` to point somewhere else. Without SVDs,
everything still works except field decoding: `load` returns a warning and
`regs` returns raw words.

### 4. Register it with your agent

Claude Code:

```bash
claude mcp add simbridge -- /path/to/simbridge/target/release/simbridge
```

Claude Desktop (**Settings → Developer → Edit Config**, then fully quit and
reopen the app):

```json
{
  "mcpServers": {
    "simbridge": {
      "command": "C:\\path\\to\\simbridge\\target\\release\\simbridge.exe",
      "env": { "SIMBRIDGE_WORKDIR": "C:\\path\\to\\simbridge\\work" }
    }
  }
}
```

`claude mcp add` only registers the server with Claude Code; Claude Desktop
reads its own config file. Then ask something like: *"Load
firmware/stm32f4-hello's release ELF on stm32f4, run 50 ms, and show the
serial output and the decoded USART2 status register."*

## Tools

Every result is JSON with `ok`, `virtual_time_s` (the machine's elapsed virtual
time after the call) and `wall_ms` (the host time the call took).

| tool | what it does |
|---|---|
| `load(elf, platform="stm32f4", uart?, cpu?, svd?)` | Creates a fresh machine and loads the ELF. `platform` is a preset (table below) or a `.repl` path, which then needs `uart` (and `cpu` if it isn't `sysbus.cpu`). `svd` overrides the preset's SVD (a path, a preset name, or `"none"`); it's parsed once, cached, and also given to Renode with `ApplySVD`. |
| `reset()` | Rebuilds the machine and reloads the same ELF, so virtual time goes back to 0. Traces stay enabled. |
| `run_for(virtual_ms)` | Advances exactly that much virtual time, then pauses. Returns the PC. |
| `run_until(uart_regex?, mem_addr?, mem_equals?, timeout_ms=1000, step_ms=1)` | Runs in `step_ms` slices until UART output matches the regex, or until the 32-bit word at `mem_addr` (address or ELF symbol) equals `mem_equals`, or until the timeout. Returns `hit`, `stop` (the reason), `elapsed_virtual_ms` and the UART text it saw. That text is consumed, so `serial()` won't return it again. The stop time is accurate to one step, and each step costs about 15–25 ms of wall time. |
| `serial(max_bytes=4096)` | Returns UART output since the previous call. |
| `regs(peripheral, count=8, offset=0, register?, verbose=false)` | Reads 32-bit words from a peripheral's bus window. With an SVD, each word is decoded into its register(s) and fields (bit range, value, matching enum name); `register="SR"` reads one register by name, `verbose` adds descriptions. The Renode peripheral is matched to the SVD by base address, so `gpioPortA` finds `GPIOA`. These are model reads, so registers with read side effects are affected (flagged as `read_side_effect` when the SVD says so). |
| `svd(svd?, peripheral?, register?, value?, query?, limit=40)` | SVD lookup that doesn't touch the simulator. No arguments: list peripherals. `peripheral`: its registers. `peripheral` + `register`: every field with bits, access, description and enumerated values; add `value` to decode a number. `query`: regex search over names and descriptions. Uses the loaded machine's SVD unless `svd` names a path or preset. |
| `mem(addr, length=64)` | Reads up to 1 KiB. `addr` can be a number, a `"0x..."` string or an ELF symbol such as `"TICKS"`. |
| `peripheral_trace(peripheral, limit=50, disable=false, decode=false)` | The first call turns on `LogPeripheralAccess`. Later calls return the accesses since the previous call: PC, R/W, offset, register name and value, plus the CPU on multi-core boards. `decode` adds the SVD field breakdown of each value. |
| `snapshot()` / `restore(id)` | Renode `Save` / `Load` of the full state. Each reports how long it took. |

## Board presets

| preset | Renode platform | core | console | SVD | test firmware target |
|---|---|---|---|---|---|
| `stm32f4` | `stm32f4.repl` | Cortex-M4F | `usart2` | `STMicro/STM32F407.svd` | `thumbv7em-none-eabihf` |
| `fe310` / `hifive1` | `sifive-fe310.repl` | RV32IMAC | `uart0` | `e310x/e310x.svd` | `riscv32imac-unknown-none-elf` |
| `nrf52840` | `nrf52840.repl` | Cortex-M4F | `uart0` (UARTE0) | `Nordic/nrf52840.svd` | `thumbv7em-none-eabihf` |
| `stm32f103` / `bluepill` | `stm32f103.repl` | Cortex-M3 | `usart2` | `STMicro/STM32F103xx.svd` | `thumbv7m-none-eabi` |
| `stm32f072` | `stm32f072.repl` | Cortex-M0 | `usart2` | `STMicro/STM32F072x.svd` | `thumbv6m-none-eabi` |
| `stm32l552` | `stm32l552.repl` | Cortex-M33 | `usart2` | `STMicro/STM32L552.svd` | `thumbv8m.main-none-eabihf` |
| `k210` | `kendryte_k210.repl` | 2× RV64IMAC | `uart` (UARTHS) | `Kendryte-Community/k210.svd` | `riscv64imac-unknown-none-elf` |

Renode `.repl` files live under `platforms/cpus/`; SVD paths are under
`cmsis-svd-data/data/` except `e310x`. The K210 has two cores (`cpu1`,
`cpu2`); the preset reports `cpu1`'s PC, and the test firmware parks hart 1.
Renode and SVD peripheral names often differ (`uart` vs `UARTHS`, `gpioPortA`
vs `GPIOA`), so simbridge matches them by base address.

Any other Renode platform works without a preset: pass a `.repl` path as
`platform`, plus `uart`, and `svd` if you have one.

### Adding a preset

Presets are one table in `src/presets.rs`. Each entry gives the Renode
platform, the console UART, the CPU to report, the SVD's path relative to the
SVD folder, and optional `svd_alias` entries for when the SVD has several views
at one address and the Renode model matches a specific one. To have the smoke
test cover the new board, add a `firmware/<board>-hello` crate and an entry in
`examples/smoke.rs`. The existing hello crates are about 40 lines each and share
their banner and tick loop through `firmware/common/hello.rs`.

## Testing

```bash
rustup target add thumbv6m-none-eabi thumbv7m-none-eabi thumbv7em-none-eabihf \
  thumbv8m.main-none-eabihf riscv32imac-unknown-none-elf riscv64imac-unknown-none-elf
for d in firmware/*-hello; do (cd $d && cargo build --release); done
cargo run --release --example smoke            # every board
cargo run --release --example smoke -- k210    # just one
```

In PowerShell, the firmware loop is:
`Get-ChildItem firmware -Directory -Filter *-hello | % { Push-Location $_; cargo build --release; Pop-Location }`

`examples/smoke.rs` rebuilds the server binary, spawns it, and drives every
tool over real MCP stdio on every preset board. It checks:

- the UART banner;
- that the trace isn't empty;
- that `regs` and `svd` return decoded fields;
- that `restore` rewinds the `TICKS` counter in RAM;
- `run_until` stopping on a UART regex, on a memory value and on its timeout.

If an MCP client is currently running the server, Windows locks the binary and
the rebuild step fails, so close the client first.

`cargo test` runs the unit tests: the SVD parser and decoder, plus trace-line
parsing. Set `SIMBRIDGE_TEST_SVD_DIR` to a folder of `.svd` files to also check
that each of them parses.

## Configuration

| variable | default | purpose |
|---|---|---|
| `SIMBRIDGE_RENODE` | `PATH`, then `C:\Program Files\Renode\renode.exe` or `/opt/renode/renode` | Renode executable |
| `SIMBRIDGE_SVD_DIR` | first `svd` folder next to the executable or in a parent folder, then `./svd` | root for preset SVD paths and relative `svd` arguments |
| `SIMBRIDGE_WORKDIR` | system temp dir | where Renode's log and snapshots go; each simbridge process uses its own `simbridge-<pid>` subfolder |
| `SIMBRIDGE_TOOLS` | all tools | which tools are exposed: tool names and/or the feedback conditions `C2`, `C3`, `C4`, separated by commas or spaces. Hidden tools are neither listed nor callable. An unknown name stops the server at startup |
| `SIMBRIDGE_LOG` | off | append one JSON line per tool call to this file (see below) |
| `SIMBRIDGE_MODE` | `agent` | `grader` adds grader-only tools (`monitor`: a raw Renode monitor command) for benchmark graders. In agent mode they don't exist, and `SIMBRIDGE_TOOLS` can't expose them |

### Feedback conditions and the call log

For benchmarking agents, `SIMBRIDGE_TOOLS` restricts what the agent can observe:

| condition | tools |
|---|---|
| `C2` | `load`, `reset`, `run_for`, `run_until`, `serial` |
| `C3` | C2 + `regs`, `mem`, `svd` |
| `C4` | C3 + `peripheral_trace`, `snapshot`, `restore` (everything) |

`SIMBRIDGE_LOG` records the simulator side of a run. The first line is a start
event listing the exposed tools; then each call adds one line:

```json
{"event":"call","seq":3,"t_unix_ms":1794283325521,"t_wall_ms":65.7,"tool":"run_for",
 "args":{"virtual_ms":50},"ok":true,"t_virtual_s":0.05,"wall_ms":93.4,"result_bytes":61}
```

`result_bytes` is the size of the text returned to the agent, i.e. what that
call added to its context. Calls to hidden tools are logged with `ok: false`.
The log can't see the agent's tokens or context length; those come from the
agent's own transcript, joined to this log on `t_unix_ms`.

## How it works

```text
MCP client ──stdio──▶ simbridge ──monitor socket──▶ renode --disable-xwt --plain --port N
                          │  ◀── server-socket terminal (UART bytes)
                          │  ◀── renode.log (LogPeripheralAccess lines)
                          └── SVD index (svd-parser, parsed once, cached)
```

simbridge starts Renode on the first tool call and drives it with monitor
commands. UART output comes from a server-socket terminal that simbridge
connects to. Traces come from `LogPeripheralAccess` lines in Renode's log file,
read from where the previous call left off. The SVD is parsed with
[`svd-parser`](https://crates.io/crates/svd-parser), with `derivedFrom`,
arrays and inherited properties expanded and clusters flattened, then cached.
`regs` and trace decoding look up each value's register in that index. Renode
stops when the client disconnects. On Windows it also runs in a kill-on-close
job object, so it can't outlive a crashed or killed simbridge.

It uses no pyrenode3 or .NET interop, just text commands over a socket.

## Measured (Windows, Renode 1.17.0)

| | STM32F4 | FE310 | nRF52840 | K210 |
|---|---|---|---|---|
| snapshot (Save) | ~0.7 s, 4.4 MB | ~0.1 s, 0.9 MB | ~0.3 s, 3.9 MB | ~0.05 s, 9.7 MB |
| restore (Load + UART reattach) | ~1.2 s | ~0.3 s | ~0.5 s | ~0.3 s |
| run_for 50 ms virtual | ~65–95 ms wall | ~65 ms wall | ~65 ms wall | ~70 ms wall |

Snapshot size and time depend on how much memory the platform maps: STM32F4
has 2 MiB of flash plus a 256 MiB FSMC window, and the K210 has 32 MiB of RAM.
The first call also pays Renode's cold start, about 1–5 s.

## Limits

- **Models are simplified.** Renode's peripheral models implement a subset of
  each peripheral, and some behave differently from the silicon. For example,
  the STM32 USART model always reports TXE = 1, and on the nRF52840 only the
  EasyDMA UART (UARTE) is modeled. Passing in simulation doesn't prove the
  firmware works on hardware.
- **Not cycle-accurate.** Virtual time is deterministic but not cycle-exact, and
  there's no analog, RF or sensor physics.
- **Renode's boards only.** A chip without a Renode platform can't be run. For
  example, there's no upstream RP2040 or ESP32 platform. Their SVDs still work
  with the `svd` lookup tool.
- **SVD data is only as good as the vendor's.** See the notes below. When an
  SVD and the reference manual disagree, trust the manual.
- **One machine per server instance,** and tools run one at a time.

## Notes

- The nRF52840 preset maps `uart0`/`uart1` to the SVD's `UARTE0`/`UARTE1`:
  Renode's `nrf52840.repl` builds them with `easyDMA: true`, which removes the
  legacy UART's `TXD` register. The test firmware therefore transmits through
  EasyDMA (`TXD.PTR`/`TXD.MAXCNT`/`TASKS_STARTTX`, wait for `EVENTS_ENDTX`).
  Renode's model also logs `EVENTS_ENDTX` under the name `EVENTS_ENDRX`.
- On Windows, Renode keeps UART file backends locked, so UART output is read
  from a `CreateServerSocketTerminal`. Terminals aren't saved in snapshots, so
  `restore` reattaches one.
- `mach clear` only deselects the current machine; `load` uses `Clear`.
- In the trace log, Renode tags lines `usart2:` when there is one machine and
  `machine/usart2:` when there are several, and writes `[cpu: …]` on
  single-core platforms but `[cpu1: …]` on multi-core ones. The parser accepts
  all of these.
- SVD quirks: Nordic writes `read-writeonce` where the schema says
  `read-writeOnce`; simbridge normalizes the case of enum-valued tags before
  parsing, because `svd-parser` is strict about it.
- SVD data quality varies. ST's own `STM32F407.svd` gives USART `SR` a reset
  value of `0xC00000` (the reference manual says `0x00C0`) and marks clear-by-
  writing-0 flags as plain `rw`. The patched SVDs that
  [`stm32-rs`](https://github.com/stm32-rs/stm32-rs) generates fix many such
  errors; point `svd` at one of those when it matters.
- `ApplySVD` only names registers in traces for regions Renode doesn't model;
  the built-in STM32 models already name theirs. Field decoding is done by
  simbridge itself and doesn't depend on it.
- RV64 firmware linked with `riscv-rt` 0.13 at `0x80000000` must discard
  `.eh_frame` (see `firmware/k210-hello/memory.x`). Otherwise the linker fails
  with `R_RISCV_32_PCREL out of range`, because `link.x` places the unwind
  tables at address 0.

## Repository layout

```text
src/main.rs          MCP tool definitions (rmcp)
src/renode.rs        Renode process, monitor protocol, UART tap, traces, snapshots
src/svd.rs           SVD parsing, index, field decoding, search
src/presets.rs       board presets
src/calllog.rs       SIMBRIDGE_TOOLS allowlist and SIMBRIDGE_LOG call log
examples/smoke.rs    end-to-end test over MCP stdio, all boards
firmware/            per-board hello firmware + common/hello.rs
bench/               simbridge-bench: grading client, task format, validator, tasks
python/              the first prototype in Python, kept for reference
```

## License

MIT; see [LICENSE](LICENSE). Renode, the SVD files and the crates simbridge
depends on keep their own licenses.
