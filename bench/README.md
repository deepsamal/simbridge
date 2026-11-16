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

Needs Python 3.8+ (standard library only), the Rust target in each task's
`task.json` (`rustup target add thumbv7em-none-eabihf` for the current tasks),
and a release build of simbridge (`cargo build --release` in the repo root,
with no MCP client holding the binary open on Windows). Set `SIMBRIDGE_EXE` to
use another simbridge binary.

```bash
# Check every task's grader against its reference and its faults
python bench/validate.py
python bench/validate.py bench/tasks/T1-tim2-tick-stm32f4   # one task

# Grade one ELF (exit code 0 = pass); writes result.json
python bench/grading.py bench/tasks/B1-uart-banner-stm32f4 path/to/firmware.elf --out result.json

# Build and grade on different machines
python bench/validate.py --build-only              # ELFs into bench/target/elfs/
python bench/validate.py --prebuilt DIR            # grade <task>-<variant>.elf files in DIR
```

Each variant builds in its own target directory under `bench/target/`
(ignored by git), so the first validation run compiles dependencies once per
variant. Extra cargo arguments can be passed in `VALIDATE_CARGO_ARGS`.

A result looks like:

```json
{"task": "B1-uart-banner-stm32f4", "passed": false, "score": 0.3333,
 "checks": [{"name": "banner", "passed": false, "weight": 2, "detail": {...}}, ...],
 "error": null, "wall_s": 3.1}
```

`passed` requires every check; `score` is the weighted fraction passed.

## Writing a task

1. Copy the closest existing task folder and rename it (`<id>-<what>-<board>`).
2. Write `spec.md` for the agent: what to build, the board, the build command,
   and anything the grader reads (symbol names, output format). Nothing about
   how it's graded beyond that.
3. Write `hidden/reference/` and make it pass. Write `hidden/grade.py`:
   `CHECKS = {name: weight}` and `grade(sb, elf, g)`. Read traces before calling
   `regs`/`mem` (model reads show up in traces), and filter traces with
   `op`/`offset` rather than pulling every access.
4. Add each seeded fault as `hidden/faults/<id>/` with the replaced files and a
   `fault.json` (class, peripheral, register, field, location, description,
   symptom). A fault must build without warnings: a warning such as "unused
   import" points straight at the bug.
5. Run `python bench/validate.py bench/tasks/<id>` until it says VALID, run
   each fault once to see its real symptom, and put that symptom in
   `fault.json` and the design doc.
6. Check the simulator, not just your grader: if Renode models something
   differently from the silicon (see the main README's Limits), grade what the
   hardware would do, as T1 does for the timer period.

## Tasks

| id | family | board | seeded faults |
|---|---|---|---|
| B1-uart-banner-stm32f4 | F1 bring-up | stm32f4 | X1: polls RXNE instead of TXE |
| T1-tim2-tick-stm32f4 | F2 timers & interrupts | stm32f4 | X2: tick read not volatile; X3: UIF not cleared; X4: PSC 16 not 15; X5: NVIC not enabled |
