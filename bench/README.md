# simbridge-bench

Tasks and grading for measuring how well, and how efficiently, coding agents
develop and repair MCU firmware firmware-in-the-loop. The design (task
families, feedback conditions C0–C5, metrics) is in the project's design doc;
this folder holds the code that runs it.

## Layout

```text
bench/
  simbridge_client.py   MCP stdio client for simbridge (standard library only)
  grading.py            Grade/check helpers + CLI: grade one ELF against one task
  validate.py           reference must pass, every seeded fault must fail
  tasks/<id>/
    task.json           id, family, board, Rust target, package name, budgets
    spec.md             what the agent reads
    workspace/          what the agent starts from (builds as-is)
    hidden/             never shown to the agent
      grade.py          defines CHECKS {name: weight} and grade(sb, elf, g)
      reference/        known-good solution
      faults/<id>/      files that replace the reference's to plant one fault,
                        plus fault.json (ground truth: class, register, location)
```

Task crates use a `build.rs` for the linker script instead of
`.cargo/config.toml`, so the Rust target is given on the command line:
`cargo build --release --target <target from task.json>`.

## Grader mode

Graders drive their own simbridge with `SIMBRIDGE_MODE=grader`, which adds
grader-only tools (today: `monitor`, a raw Renode monitor command). In the
default agent mode those tools don't exist and no `SIMBRIDGE_TOOLS` setting
can expose them. The grader's instance is separate from the agent's, so
nothing the agent does can reach or influence grading.

## Commands

Build simbridge first (`cargo build --release` in the repo root), with no MCP
client holding the binary open.

```bash
# Check every task's grader against its reference and its faults
python bench/validate.py

# Grade one ELF (exit code 0 = pass); writes result.json
python bench/grading.py bench/tasks/B1-uart-banner-stm32f4 path/to/firmware.elf --out result.json
```

Each variant builds in its own target directory under `bench/target/`
(ignored by git), so the first validation run compiles dependencies once per
variant.

A result looks like:

```json
{"task": "B1-uart-banner-stm32f4", "passed": false, "score": 0.3333,
 "checks": [{"name": "banner", "passed": false, "weight": 2, "detail": {...}}, ...],
 "error": null, "wall_s": 3.1}
```

`passed` requires every check; `score` is the weighted fraction passed.

## Tasks

| id | family | board | seeded faults |
|---|---|---|---|
| B1-uart-banner-stm32f4 | F1 bring-up | stm32f4 | X1: polls RXNE instead of TXE |
| T1-tim2-tick-stm32f4 | F2 timers & interrupts | stm32f4 | X2: tick read not volatile; X3: UIF not cleared; X4: PSC 16 not 15; X5: NVIC not enabled |
