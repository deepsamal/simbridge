# simbridge-bench: Design Doc

Nov 9, 2026 · Deep Samal · last updated Nov 15, 2026

> This is the version-controlled copy of the design. Change it through pull
> requests like any other file. Status as of the last update: simbridge has
> every change needed before the first run (allowlist, call log, grader mode,
> virtual-time timestamps), and tasks **B1** and **T1** are built and validated
> (see [`bench/`](../bench)). Next: B2, B3, T3, then the M1 pilot run.

## Goal and research questions

simbridge-bench measures how well, and how efficiently, coding agents develop and repair register-level microcontroller firmware when they can test it firmware-in-the-loop: the real compiled firmware running on a deterministic simulated MCU (simbridge on Renode). It grades the final firmware automatically, and it also records what the agent spent to get there: iterations, tool calls, tokens, context and time.

It answers four questions:

1. **RQ1 Capability.** On which kinds of firmware work (bring-up, interrupts, bug fixing, porting, protocols) do current agents succeed, and on which do they fail?
2. **RQ2 Feedback.** How much does each kind of simulator feedback help: pass/fail only, UART output, decoded registers and SVD lookups, bus traces, snapshots? Embedded Arena showed that hardware feedback matters for ML deployment; this isolates which feedback matters for firmware correctness.
3. **RQ3 Efficiency.** For tasks that are solved, what does a solution cost in iterations, tokens, peak context and wall time, and which feedback lowers that cost?
4. **RQ4 Grounding.** How often do agents write to registers or bits that don't exist on the chip, and does an SVD lookup tool reduce that?

Scope for v0: bare-metal and HAL-level firmware in Rust and C on seven Renode boards. Out of scope: power, thermal and cycle-accurate timing, which need real hardware.

## Related work and the gap

The closest benchmarks grade outcomes on hobbyist boards or on real hardware. None of them measures iterative, register-level work on industrial MCUs, or what that work costs the agent.

| Benchmark | Workload | Backend | Interaction | Reports | Reusable |
| --- | --- | --- | --- | --- | --- |
| [EmbedBench](https://arxiv.org/abs/2506.11003) (ICSE 2026) | 126 cases × 3 settings: write code, design circuit and code, port between boards; 9 components | Wokwi simulator, Arduino Uno / ESP32 / Pico | Single shot, plus compiler-feedback and RAG variants | pass@1 | [Dataset](https://huggingface.co/datasets/xhwl/EmbedBench), Apache-2.0 |
| [IoT-SkillsBench](https://arxiv.org/abs/2603.19583) | 42 tasks, 3 difficulty levels, 23 peripherals, 3 platforms | Real hardware | Agent, with no skills, LLM-written skills or expert skills | Success rate over 378 runs | No code or data linked |
| [Embedded Arena](https://arxiv.org/abs/2606.16190) | Fit ML models to MCUs: detection, vision compression, audio | Real MAX78000 and STM32N6 | Multi-turn, hardware / docs / score-only feedback | Accuracy, compression, deployment success | [Code](https://github.com/ubicomplab/embedded-arena) |
| simbridge-bench (this doc) | Bring-up, bug fixing, diagnosis, porting, protocols on 7 Renode boards | Renode, deterministic | Multi-turn agent over MCP | Outcome **and** cost, plus register-level grounding | Planned open |

What simbridge-bench adds:

- **Register-level tasks on industrial MCUs** (STM32, nRF52, RISC-V), not Arduino sketches.
- **Efficiency as a first-class result:** iterations, tool calls, tokens, peak context and wall time per solved task. Determinism makes these comparable across runs.
- **A feedback ladder** that adds one kind of simulator feedback at a time, so the effect of each is measured.
- **Bug fixing and diagnosis**, which none of the three cover.
- **Grounding measured from the bus:** writes to registers that don't exist, read-only registers or reserved bits are counted directly from traces against the SVD.

## Task families

Nine families, each defined by what the agent is given, what it must hand back, and how the grader decides. Bug fixing and diagnosis are separate families: a fix is graded by the firmware's behaviour after the change, a diagnosis by a written root-cause report.

| ID | Family | Agent is given | Agent delivers | Graded by |
| --- | --- | --- | --- | --- |
| F1 | Bring-up | Spec, board, empty crate or C project | Working driver (UART, GPIO, timer, SPI/I²C to a modeled sensor) | Hidden behavioural tests + bus trace shows the real peripheral used |
| F2 | Timers and interrupts | Spec with rates and tolerances | Periodic IRQ, PWM, debounced state machine | Event timestamps in virtual time vs spec |
| F3 | **Bug fix** | Source + build that misbehaves, a bug report (symptom only) or a failing test | A patch | Hidden tests pass, no regressions, patch size; localization scored too |
| F4 | Porting | Working project on board A | Same behaviour on board B (different peripheral layout or ISA) | The same hidden tests on board B |
| F5 | Protocols | Spec of a wire protocol | Command shell, Modbus RTU slave, framing + CRC | Scripted and fuzzed host traffic over UART |
| F6 | DMA and concurrency | Spec | DMA-driven TX/RX, ISR-to-main handoff | Data integrity under load; trace shows DMA, not polling |
| F7 | Boot and memory | Spec | Bootloader with image check and jump; flash driver | Good, corrupted and truncated images; flash read back |
| F8 | **Diagnosis only** | A misbehaving ELF and a symptom; source withheld in the hard variant | A structured root-cause report | Report matched to the known fault: peripheral, register, fault class, location |
| F9 | Optimization | Correct firmware | Smaller or faster firmware, same behaviour | Code and RAM size, executed instructions; hidden tests still pass |

**F3 vs F8.** F3 asks for a working fix; an agent can pass by trial and error without understanding the fault, which is why F3 also scores localization (did the patch touch the faulty lines). F8 asks only for the explanation, so it measures understanding directly and cannot be passed by trial and error. Running the same injected faults through both shows whether agents fix bugs they can't explain, or explain bugs they can't fix.

F3 has two variants per fault: **F3a symptom-only** ("nothing is printed after reset"), closest to a real bug report, and **F3b failing-test**, where the agent sees one failing test from the hidden suite.

## v0 task suite

v0 has 21 task definitions and 37 graded instances across five families and five boards, all in Rust. Bug-fix and diagnosis tasks share eight seeded faults, so each fault is run three ways (F3a, F3b, F8). Tasks marked **needs** depend on a simbridge feature that doesn't exist yet (section on simbridge changes).

**Design rule for every task:** the reference solution passes the grader and each seeded fault fails it, checked automatically before a task is accepted. Renode doesn't model everything (it ignores clock gating and baud rates), so a fault that only breaks real hardware is not allowed in v0.

### F1 Bring-up (6)

| ID | Board | Task | Pass criteria |
| --- | --- | --- | --- |
| B1 | stm32f4 | USART2 banner at 115200 baud from 16 MHz HSI | Banner on UART; BRR = 0x8B, CR1 has UE and TE; trace shows DR writes |
| B2 | nrf52840 | UARTE0 line output through EasyDMA | Banner on UART; trace shows TXD.PTR / TASKS\_STARTTX, no legacy TXD |
| B3 | fe310 | UART0 banner with TX FIFO-full handling | Banner; txctrl.txen = 1; trace polls txdata.full |
| B4 | stm32f4 | Read a TMP108 over I²C1 (addr 0x48), print °C once a second | Prints 23.5 then 40.0 after the grader changes the sensor. **Needs** sensor stimulus |
| B5 | nrf52840 | Read a TI LM74 over SPI, print °C | Same as B4 over SPIM. **Needs** sensor stimulus |
| B6 | stm32f4 | UART echo: return each received line in upper case | 20 scripted lines, including empty and 128-byte lines. **Needs** UART input |

### F2 Timers and interrupts (4)

| ID | Board | Task | Pass criteria |
| --- | --- | --- | --- |
| T1 | stm32f4 | TIM2 update interrupt at 1 kHz; print the count every 1000 ticks | Prints at 1.000 s virtual intervals ± 0.2%; PSC/ARR give exactly 1 kHz with the hardware formula; ISR is the source (trace). Built and validated 2026-11-14 |
| T2 | nrf52840 | RTC0 at 8 Hz toggles LED1 (P0.13) | GPIO OUT edges every 125 ms ± 1 ms; no busy loop (CPU sleeps in WFI) |
| T3 | fe310 | CLINT machine-timer tick at 100 Hz | Tick count in RAM = 100 ± 1 after 1 s virtual |
| T4 | stm32f4 | Count button presses with 20 ms debounce | 10 presses with 5 ms bounce → count 10. **Needs** GPIO input |

### F3 Bug fix and F8 Diagnosis: eight seeded faults

Each fault is planted in the reference solution of the task named. F3a gives the symptom only; F3b adds one failing test; F8 gives the ELF and the symptom and asks for a root-cause report (source withheld for the four marked **no src**).

| ID | Base task | Fault | Class | Symptom given to the agent |
| --- | --- | --- | --- | --- |
| X1 | B1 | Polls RXNE instead of TXE before writing DR | Wrong register bit | Nothing is printed after reset |
| X2 | T1 | Tick counter shared with the ISR is not `volatile` (read once by the optimizer) | Concurrency / compiler | Start line prints, then no count lines; TICKS keeps counting |
| X3 | T1 | ISR doesn't clear TIM2 UIF | Missing flag clear | Board appears hung after reset (no src) |
| X4 | T1 | Prescaler off by one (PSC = 16 instead of 15) | Configuration arithmetic | Prints drift slower than 1 s (about 1.06 s apart) |
| X5 | T1 | TIM2 interrupt enabled in DIER but not in the NVIC | Missing enable | Count stays at 0 (no src) |
| X6 | B4 | 8-bit I²C address 0x90 used where 7-bit 0x48 is expected | Address encoding | Prints "sensor not found" |
| X7 | B2 | `memory.x` RAM length larger than the chip's RAM; stack starts past the end | Memory map | Crashes before the banner (no src) |
| X8 | T2 | LED on P0.14 instead of P0.13 (wrong pin constant) | Wrong peripheral instance | LED1 never blinks (no src) |

### F4 Porting (3)

| ID | From | To | What changes | Pass criteria |
| --- | --- | --- | --- | --- |
| P1 | B1 on stm32f4 | stm32f072 | USART register layout (SR/DR → ISR/TDR), Cortex-M0 | B1's tests on F072 |
| P2 | B1 on stm32f4 | nrf52840 | Polled UART → EasyDMA UARTE | B2's tests |
| P3 | T3 on fe310 | k210 | RV32 → RV64, two harts, UARTHS | T3's tests on K210; hart 1 parked |

v0 without the **needs** tasks (B4, B5, B6, T4) (and X6, which builds on B4) still has 16 task definitions and 30 instances, enough for a first run while those features are built.

## Task format and grading

Each task is a directory with an agent-visible half and a hidden half. The grader runs its own simbridge instance after the agent finishes, so the agent can never call or read it.

```text
tasks/X2-volatile-tick/
  task.yaml        id, family, board preset, budgets, which conditions apply
  spec.md          what the agent reads: goal, board, symptom (F3/F8)
  workspace/       what the agent starts from: empty crate, faulty source, or ELF only (F8 no-src)
  hidden/
    grade.py       drives simbridge: load, stimulus, run, checks -> result.json
    reference/     the known-good solution
    fault.patch    reference -> faulty (F3/F8)
    fault.json     ground truth: file, lines, peripheral, register, field, fault class
```

In the code, `task.yaml` is `task.json` (Python's standard library reads JSON, not YAML), and each fault is a folder `hidden/faults/<id>/` holding the files that replace the reference's plus `fault.json`, instead of a `fault.patch` (see [`bench/README.md`](../bench/README.md)). It holds what the grader and validator need: id, family, board, Rust target, package, budgets, trials. The Embedded Arena-style YAML for the runner can be generated from it:

```yaml
id: X2-volatile-tick
family: F3a
board: stm32f4
base_task: T1
budget: {max_iterations: 20, max_tool_calls: 150, max_wall_min: 30, max_tokens: 2000000}
trials: 3
checks:
  - {name: builds,       weight: 0}
  - {name: periodic_1s,  weight: 1}
  - {name: isr_drives,   weight: 1}
  - {name: no_regress,   weight: 1}
```

**Grader checks come in three kinds:**

1. **Behavioural:** UART text, event timestamps in virtual time, RAM values, responses to scripted input.
2. **Structural:** the bus trace and register state, which prove the behaviour came from the right peripheral (B1 must write USART2 DR, B2 must start an EasyDMA transfer).
3. **Patch (F3 only):** regressions on the base task's full suite, diff size, and whether the diff touches the lines in `fault.json`.

**Diagnosis reports (F8)** are JSON, so they're graded without an LLM judge:

```json
{"peripheral": "TIM2", "register": "SR", "field": "UIF",
 "fault_class": "missing_flag_clear", "location": "src/main.rs:41",
 "explanation": "ISR returns without clearing UIF, so it re-enters forever"}
```

Score = 0.4 fault class + 0.3 register/field + 0.3 location (location dropped for no-src variants and re-weighted); `explanation` is kept for analysis only.

**Anti-gaming rules:**

- Hidden inputs differ from any example in `spec.md` (other temperatures, other line lengths, other press counts).
- Structural checks fail firmware that prints the expected text without driving the peripheral, uses semihosting, or reads the grader's stimulus out of band.
- The agent's sandbox has no read access to `hidden/`; F8 no-src workspaces contain only the ELF, with symbols but no source or line-level debug info.
- Every run starts from a clean workspace copy; nothing persists between trials.

## Feedback conditions and agent setup

Every task runs under six conditions that each add one kind of feedback to the one before, so the difference between neighbours is the value of that feedback. Conditions are enforced by a tool allowlist in simbridge, not by instructions in the prompt.

| Condition | Agent can use | Comparable to |
| --- | --- | --- |
| C0 Compile only | Shell, editor, `cargo build`; no simbridge | EmbedBench with compiler feedback |
| C1 Pass/fail | C0 + a `check` tool that returns pass or fail for the visible test only | Embedded Arena score-only |
| C2 Console | C1 + `load`, `run_for`, `run_until`, `serial`, `reset` | A board with a serial monitor |
| C3 Registers | C2 + `regs`, `mem`, `svd` | A debugger with an SVD viewer |
| C4 Bus trace | C3 + `peripheral_trace`, `snapshot`, `restore` | Beyond what real hardware offers |
| C5 Skill | C4 + the `svd-registers` skill in context | Expert guidance, as in IoT-SkillsBench |

F8 diagnosis runs from C2 up, since a diagnosis needs something to observe.

**Agent setup, fixed across conditions:**

- One agent harness for all models (start with Claude Code headless; Embedded Arena's provider layer covers Anthropic, OpenAI, Google and Ollama models).
- The same system prompt, workspace, toolchain (Rust stable + all six targets) and budgets per task.
- Reasoning effort fixed per model and recorded.
- 3 trials per task × condition × model, so pass@1 and pass@3 can both be reported.

For v0 that is (29 instances × 6 conditions + 8 F8 instances × 4 conditions) × 3 trials = 618 runs per model.

## Metrics

Every run reports outcome, cost and process metrics. Cost is computed only over solved runs, alongside the solve rate, so a cheap failure never looks efficient.

| Group | Metric | How it's computed |
| --- | --- | --- |
| Outcome | pass@1, pass@3 | Fraction of trials where every hidden check passes |
| Outcome | Partial score | Weighted fraction of hidden checks passed |
| Outcome | Regressions | Base-task checks that passed at the start of an F3 run and fail at the end |
| Outcome | Diagnosis score (F8) | Class / register / location match, as in the grading section |
| Cost | Wall time | Agent start to final answer |
| Cost | Iterations | Build-then-run cycles: a `cargo build` followed by a `load` |
| Cost | Tool calls | Total and per tool (`run_for`, `regs`, `svd`, `peripheral_trace`, `restore`, shell, edit) |
| Cost | Tokens and dollars | Input, output and cached tokens from the provider; dollars at list price |
| Cost | Peak context | Largest prompt size in any single model call |
| Cost | Virtual time used | Sum of `run_for` / `run_until` time; independent of host speed |
| Milestones | Time and tokens to first build, first UART output, first passing visible test | Timestamps in the run log |
| Process | Register hallucination rate | Share of the firmware's peripheral writes that hit an address not in the SVD, a read-only register, or reserved bits (from the grader's trace) |
| Process | Lookup before write | Share of distinct registers the agent's code touches that it looked up with `svd` or `regs` first |
| Process | Steps to diagnosis | Tool calls from the first observed failure to the edit that fixes it (F3) or the report (F8) |
| Quality | Code and RAM size | `.text` and `.data + .bss` from the final ELF |
| Quality | Executed instructions | Renode instruction count over a fixed workload |
| Quality | Warnings and `unsafe` | Compiler and clippy warnings; `unsafe` blocks in the final source |

**Where each number comes from.** Two logs are joined on Unix timestamps. The **agent transcript** (e.g. Claude Code's stream-JSON output) gives tokens, peak context, dollars, and every shell, build and edit call; simbridge never sees these. The **simbridge call log** gives every simulator call with its arguments, outcome, virtual time and `result_bytes`, the size of what the call added to the agent's context. That last field shows which tools fill the context (a 50-entry decoded trace, for example), which the transcript alone can't attribute.

The headline result is **solve rate against cost per condition**: one point per condition, showing which feedback buys the most solves per token. Results are reported per family and per board as well as overall, with 95% bootstrap intervals over trials.

## Reusable code and data

The harness can come almost entirely from Embedded Arena, and the sensor and board models from Renode. Real bug corpora exist, but mostly in network stacks that need input injection, so they fit v1 rather than v0. Licenses marked ✓ were read from the repository's LICENSE file or dataset card.

| Source | License | What it gives us | Where it's used |
| --- | --- | --- | --- |
| [Embedded Arena](https://github.com/ubicomplab/embedded-arena) | MIT ✓ | Python runner: task YAML (instructions, iterations, trials, max\_tool\_calls), checks with a per-check `feedback` flag, sandbox, provider adapters for Claude / OpenAI / Gemini / Ollama | Harness. Its `feedback` flag already implements condition switching; our checks replace its compile/flash/measure checks |
| [Renode](https://github.com/renode/renode) | MIT ✓ | Board platforms; 40+ sensor models incl. TMP108 (I²C) and TI LM74 (SPI); Robot Framework tests | Boards and sensors for B4/B5; test scripts as patterns for `grade.py` |
| [EmbedBench](https://huggingface.co/datasets/xhwl/EmbedBench) | Apache-2.0 ✓ | 126 problems with reference code and Wokwi tests (LEDs, buttons, 7-segment, shift registers) | v1 F1/F2 specs, ported to Renode GPIO LED/button models; its migration setting informs F4 |
| [P2IM unit tests](https://github.com/RiS3-Lab/p2im-unit_tests) | None stated — ask the authors | Source, STM32F103 ELFs and per-test CSVs of every register accessed, for ADC, GPIO, I²C, SPI, PWM, UART on Arduino / NuttX / RIOT | F8 targets on the stm32f103 preset; the CSVs validate the register-hallucination metric |
| [Fuzzware experiments](https://github.com/fuzzware-fuzzer/fuzzware-experiments) | Apache-2.0 ✓ | 66 synthetic hardware-interaction unit tests; real targets with CVEs in Zephyr and Contiki-NG | v1 real-bug tier for F3/F8 (needs UART or radio input) |
| [FirmReBugger / FirmBench](https://arxiv.org/abs/2601.15774) (USENIX Security 2026) | To confirm | Real firmware binaries with 313 bug oracles | v1 F8 real-bug tier |
| [Cortex-M security study](https://par.nsf.gov/servlets/purl/10563137) | To confirm | 310 Cortex-M bug reports from 2017–2023, categorized; dataset open-sourced | Fault-class taxonomy for choosing seeded faults |
| embassy, stm32-rs, nrf-hal, e310x-hal | MIT / Apache-2.0 (to confirm per crate) | HALs, PACs and examples | Reference solutions; HAL-level task variants |
| cargo-mutants | To confirm | Mutation operators for Rust | Candidate fault generation; kept only if the differential check shows a visible failure |
| SWE-bench | To confirm | Patch-based task format, fail-to-pass and pass-to-pass tests | F3 patch scoring and regression checks |

**Gaps no source fills:** none of these has register-level faults with ground truth on our boards, or efficiency logs. The eight v0 faults are written by hand from the Cortex-M bug taxonomy, and the logging is new.

## simbridge changes required

Seven additions, three of them needed before the first v0 run. The Renode mechanism listed is the likely route; each still needs a spike to confirm it behaves as expected.

| Change | Why | Blocks | Likely Renode mechanism | Before first run? |
| --- | --- | --- | --- | --- |
| Tool allowlist (`SIMBRIDGE_TOOLS`) | Enforce conditions C1–C4 | All conditions | None: simbridge hides tools it isn't allowed to serve | Done: smoke test passed on Windows, 2026-11-09 |
| Call log (`SIMBRIDGE_LOG`, JSONL) | One record of simulator use for any agent or harness; proves which tools each condition exposed; context added per call | All metrics | None: one line per tool call with args, result size, wall and virtual time | Done: smoke test passed on Windows, 2026-11-09 |
| Grader mode | `grade.py` drives a separate instance with stimulus and checks | All grading | Same binary, driven as a library or over stdio by the grader | Done: B1 validates on Windows (reference 1.0, X1 0.33), 2026-11-11. Needed a trace fix: Renode's log is now flushed per write and synced with a marker line |
| Virtual-time timestamps on UART and trace events | Timing checks for T1–T3; F2 grading | F2 | Renode log stamped in virtual time via simbridge's own --config (log-timestamps-type = Virtual); resolution = global quantum, 100 us default; serial() reports its virtual-time window | Done: smoke passed on all 7 boards on Windows, 2026-11-13; resolution 100 us confirmed |
| UART input (`uart_write`) | Echo, shells, protocols | B6, F5 | Write bytes to the socket terminal simbridge already holds | Before B6 |
| Stimulus: GPIO input and sensor values | Buttons, sensors | B4, B5, T4 | Button and sensor models added to the platform, set through their monitor properties | Before B4/B5/T4 |
| Instruction count | F9 optimization; idle detection for T2 | T2, F9 | CPU's executed-instruction counter | Before T2 |
| Trace filters and streaming (added) | A firmware spinning on a status register logs millions of accesses; graders need "every write to DR" without pulling them all | All graders | `peripheral_trace(op, offset)`; the log is streamed line by line, keeping only the newest `limit` matches | Done 2026-11-13: B1's X1 no longer runs simbridge out of memory |
| Timers at the chip's reset clock (added) | Renode clocks every STM32 timer at a fixed 10 MHz, so firmware correct on hardware times wrong | T1, T4, any STM32 timer task | Presets set each timer's `Frequency` at load (F4 16 MHz, F103 8 MHz, L552 4 MHz) | Done 2026-11-14: T1 validates |

The log line format is fixed now so every later analysis reads the same fields:

```json
{"t_wall_ms": 18234.5, "t_virtual_s": 0.115, "tool": "regs", "args": {"peripheral": "usart2", "register": "SR"}, "ok": true, "result_bytes": 812, "wall_ms": 21.4}
```

## Milestones, risks and open questions

The first milestone is the harness, M0; no dates are set yet. Each phase starts only when the previous gate passes.

| Phase | Work | Gate to the next phase |
| --- | --- | --- |
| M0 Harness | The simbridge changes above, grader mode, `validate.py`, the first tasks | Every task's reference passes and every seeded fault fails |
| M1 Pilot | B1–B3, T1, T3 and their faults; first agent runs on one model; the call log joined to the agent transcript | Grades and cost metrics come out of a real run end to end |
| M2 Full v0 run | The whole v0 suite across conditions C0–C5 and several models | Results stable across the 3 trials |
| M3 Hardware cross-check | Selected tasks on 1–2 real boards | Simulation and hardware agree on pass/fail |

M0 is the accent: it's the next piece of work, and every later phase depends on its log and grader.

**Risks**

| Risk | Effect | Mitigation |
| --- | --- | --- |
| Renode doesn't model a behaviour a fault depends on (clock gating, baud rate) | Fault invisible, task unfair | Differential check: reference passes, fault fails, before a task is accepted |
| Agents learn simulator quirks (TXE always 1) | Firmware passes in simulation, fails on silicon | M3 hardware cross-check on 1–2 real boards; quirks listed in each task's spec |
| Resolved 2026-11-13: trace lines now carry virtual time (100 us resolution). Previously: no way to timestamp trace lines in virtual time | T1–T3 timing checks can't be written | Spike in M0: Renode log options, or stepped runs with `run_until` |
| Simulator fidelity gaps found while building T1: STM32 timers fixed at 10 MHz, and `STM32_Timer` reloading after ARR counts instead of ARR + 1 | Correct firmware times wrong, or an ARR off-by-one looks exact | Presets set the timer clock; T1's grader checks the rate from PSC/ARR with the hardware formula as well as by measurement. Report the ARR bug upstream |
| Contamination: tasks leak into training data | Inflated scores later | Keep a private hold-out set of faults; rotate seeded faults per release |
| Run cost: 618 runs per model | Budget | Pilot in M1 on one model; drop conditions that show no difference |
| Grader bugs | Wrong scores | Every grader runs against the reference and every fault in CI |

**Open questions**

- [ ] Rust only for v0, or C alongside it from the start?
- [ ] Which models and reasoning levels for M2?
- [ ] Publish the task set openly, or keep part of it private as a hold-out?
- [ ] Ask the P2IM authors for permission to redistribute their unit tests.
- [ ] Which real boards for the M3 cross-check: an STM32F4 Discovery and an nRF52840 DK?

## Sources

- [EmbedAgent / EmbedBench](https://arxiv.org/abs/2506.11003) and its [dataset](https://huggingface.co/datasets/xhwl/EmbedBench)
- [Skilled AI Agents for Embedded and IoT Systems Development (IoT-SkillsBench)](https://arxiv.org/abs/2603.19583)
- [Embedded Arena](https://arxiv.org/abs/2606.16190) and its [code](https://github.com/ubicomplab/embedded-arena)
- [P2IM unit tests](https://github.com/RiS3-Lab/p2im-unit_tests)
- [Fuzzware experiments](https://github.com/fuzzware-fuzzer/fuzzware-experiments)
- [FirmReBugger](https://arxiv.org/abs/2601.15774)
- [Cortex-M security study](https://par.nsf.gov/servlets/purl/10563137)
- [Renode](https://github.com/renode/renode)
