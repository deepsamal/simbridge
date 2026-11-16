# Contributing

simbridge has three parts that change at different speeds: the MCP server
(`src/`), the skills that guide agents (`skills/`), and the benchmark that
measures both (`bench/`, designed in [`docs/design.md`](docs/design.md)). This
page covers setting up, the checks to run before a pull request, and the
conventions for each part.

## Set up

Follow the README's [Quick start](README.md#quick-start) through step 3
(Renode 1.17+, Rust with the firmware targets, Python 3.8+, the SVD folder).
Then build everything once and check it works:

```bash
cargo build --release
for d in firmware/*-hello; do (cd $d && cargo build --release); done
cargo test
cargo run --release --example smoke      # all 7 boards, a few minutes
python bench/validate.py                 # first run ~5-10 minutes (dependency builds)
```

Windows (PowerShell) firmware loop:
`Get-ChildItem firmware -Directory -Filter *-hello | % { Push-Location $_; cargo build --release; Pop-Location }`

Expected: the smoke test ends with `MCP OK`, and `validate.py` with
`all tasks valid`.

To use simbridge from an agent while developing it, register the release
binary with your MCP client (README step 4). On Windows a running client locks
`simbridge.exe`, so close it before rebuilding, or register a copy of the
binary instead of `target/release/simbridge.exe`.

## Before a pull request

| you changed | run |
|---|---|
| `src/`, `Cargo.*` | `cargo test`, the smoke test, `python bench/validate.py` |
| `src/presets.rs` or a board | the smoke test for that board: `cargo run --release --example smoke -- <preset>` |
| `bench/` | `python bench/validate.py` (or just the tasks you touched) |
| `skills/` | the skill still loads in an agent (see [`skills/README.md`](skills/README.md)) |
| docs only | check links and commands you changed actually work |

Windows and Linux both matter: simbridge has Windows-only code (process
cleanup, file sharing on Renode's log), so a change that touches process
handling or file I/O should be smoke-tested on both if you can. Say in the PR
which platforms you ran.

Keep the docs in step with the code in the same PR: the tools table and
configuration table in `README.md`, the task table in `bench/README.md`, and
the status columns in `docs/design.md`.

## Conventions

**Server (`src/`).** Tool results are JSON with `ok`, `virtual_time_s` and
`wall_ms`; add fields rather than change existing ones, since graders and the
call log read them. A tool argument that agents will use goes in the README's
tools table. Grader-only tools go in `GRADER_ONLY` in `src/main.rs` and must
never be reachable in agent mode.

**Presets.** One entry in `src/presets.rs`, a `firmware/<board>-hello` crate
built from `firmware/common/hello.rs`, and an entry in `examples/smoke.rs`.
Check the Renode platform's timer clocks against the chip's reset clock (see
the README's Limits).

**Benchmark tasks.** See [Writing a task](bench/README.md#writing-a-task). The
rule that matters most: a task is accepted only when `validate.py` says the
reference passes and every seeded fault fails. Hidden files (`hidden/`) are
never shown to an agent under test, so don't reference them from `spec.md` or
the workspace.

**Skills.** See [`skills/README.md`](skills/README.md). Skills stay generic:
no answers to benchmark tasks. Keep them in `skills/`, never in
`.claude/skills/` or `.agents/skills/` inside this repo, where agents would load
them automatically during benchmark runs.

**Files.** Shell scripts are LF (`.gitattributes` enforces it). `target/`,
`work/` and `__pycache__/` are ignored; SVD files live outside the repo (README
step 3) and are never committed.

## Where things stand

The roadmap and open questions are at the end of
[`docs/design.md`](docs/design.md). In short: the server has everything the
first benchmark run needs, tasks B1 and T1 exist, and next are tasks B2
(nRF52840 UARTE), B3 (FE310 UART) and T3 (FE310 timer tick), then a pilot
agent run. Skills beyond `svd-registers` are open: see the ideas at the end of
`skills/README.md`.
