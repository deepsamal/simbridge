# Skills

Skills are the second half of simbridge: guidance that makes coding agents
better at embedded work, as opposed to the tools that let them observe it.
Each skill is a folder with a `SKILL.md`: YAML frontmatter (`name`,
`description`) followed by Markdown instructions, in the
[Agent Skills](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview)
format that Claude Code, Claude apps and OpenCode all read. The agent sees each
skill's `description` up front and loads the body when the description fits
the task, so the description decides when a skill is used.

| skill | for | status |
|---|---|---|
| [`svd-registers`](svd-registers/SKILL.md) | Code that touches memory-mapped registers when an SVD exists: look registers up instead of guessing, decode values, verify with `regs` / `peripheral_trace` | v1, used for benchmark condition C5 |

## Why this folder isn't `.claude/skills/`

Agents discover skills automatically in `.claude/skills/`, `.agents/skills/`
and `.opencode/skills/`, searching upward from their working directory. The
benchmark compares agents *with* and *without* a skill (condition C5 against
C4, see [`docs/design.md`](../docs/design.md)), so a skill must never load by
accident. Keep skills here and install them only where you mean to use them.

## Installing a skill

Copy (or symlink) the skill's folder; the folder name must match `name:`.

| agent | per project | for your user |
|---|---|---|
| Claude Code | `<project>/.claude/skills/<name>/` | `~/.claude/skills/<name>/` |
| OpenCode | `<project>/.opencode/skills/<name>/` (also reads `.claude/skills/`) | `~/.config/opencode/skills/<name>/` |
| Claude apps (claude.ai, Desktop) | — | upload the folder as a zip in the app's skill settings |

```bash
# e.g. for one agent workspace only
mkdir -p ~/work/b1-attempt/.claude/skills
cp -r skills/svd-registers ~/work/b1-attempt/.claude/skills/
```

Restart the agent (or start a new session) after adding or changing a skill.
Check that it appears in the agent's list of available skills before testing.

## Writing and changing skills

- **Generic, never task-specific.** A skill must not contain answers to
  benchmark tasks (B1's `BRR = 0x8B`, T1's `PSC = 15`) or describe a seeded
  fault so closely that it reads as the fix. The benchmark measures whether
  guidance transfers; a skill tuned to the task set measures nothing. Use
  chips and peripherals outside `bench/tasks/` for examples where you can.
- **Every fact checked.** Bit positions, register names and tool behaviour in a
  skill are verified against the SVD (`svd` tool) or simbridge itself, not
  written from memory: the point of the skill is to stop agents doing that.
- **The description is a trigger.** Say when to use the skill in terms of the
  situation ("code that touches memory-mapped registers"), not what's inside.
- **Short.** The whole body loads into the agent's context; every line costs
  tokens on every task it triggers on. Move long reference material into files
  next to `SKILL.md` that the skill tells the agent to read when needed.
- **Version it.** Note what changed in the PR. Benchmark results are only
  comparable for the same skill text, so results should record the commit.

## Evaluating a skill

The automated runner (M1 in the design doc) isn't built yet. Until then, an
A/B comparison by hand gives a first signal:

1. Pick a task, e.g. `bench/tasks/B1-uart-banner-stm32f4`. Copy its
   `spec.md` and `workspace/` into two fresh folders, **without** `hidden/`.
2. In one folder install the skill (`.claude/skills/svd-registers/`); leave the
   other without.
3. Register simbridge for the agent with `SIMBRIDGE_LOG` set to a different
   file per attempt (and `SIMBRIDGE_TOOLS=C4`, so both attempts see the same
   tools). See the main README for client configuration.
4. Give the agent the same prompt in both: *"Implement spec.md. Build with the
   command it gives, test with simbridge, stop when it works."*
5. Grade each result and compare:

   ```bash
   python bench/grading.py bench/tasks/B1-uart-banner-stm32f4 \
       attempt-a/target/thumbv7em-none-eabihf/release/b1-firmware --out a.json
   ```

   Compare `score`, and from the call logs the number of tool calls, `svd`
   lookups before the first register write, and total `result_bytes`. Run
   each side at least three times: single runs vary a lot.

## Ideas not yet written up

Known pitfalls that came out of building the tasks and could become skill
content, once checked against more than one chip:

- Simulator fidelity: Renode clocks STM32 timers at a fixed rate and its
  `STM32_Timer` reloads after ARR counts rather than ARR + 1 (README, Limits).
  Firmware tuned to the simulator can be wrong on hardware.
- Interrupt bring-up checklist: peripheral interrupt enable, NVIC enable,
  clearing the flag in the handler, sharing state with the handler safely.
  (Careful: this overlaps T1's seeded faults; keep it generic and consider
  evaluating it on tasks outside T1.)
- Reading a bus trace to find where firmware is stuck (polling loop, PC,
  register that never changes).
