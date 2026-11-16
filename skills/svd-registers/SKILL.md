---
name: "svd-registers"
description: "Use when writing, debugging or reviewing code that touches memory-mapped peripheral registers and a CMSIS-SVD exists for the chip (via simbridge's svd/regs tools or an .svd file)."
---

# Working with peripheral registers through SVD

A CMSIS-SVD file is the vendor's machine-readable register map: peripherals,
base addresses, registers, bit fields, access rules and enumerated values. Treat
it as the source of truth for *layout*, and as a claim to verify, not ground
truth. It says nothing about *behaviour* (sequencing, timing, clock gating
dependencies, pin muxing, errata).

## Never read raw SVD XML into context

SVDs are large (STM32F407: 2.1 MB, STM32L552: 3.8 MB) and their meaning
depends on resolution: `derivedFrom` inheritance, `dim` arrays (`CH%s`),
clusters with relative offsets, and register properties (size, access, reset)
inherited from device or peripheral level. Grepping the XML gives wrong answers
for derived peripherals and arrayed registers. Always go through a parser that
expands these.

- **With simbridge:** use the `svd` tool (no simulator needed) and `regs`.
- **Without:** a short script using `svd-parser` (Rust, `expand(true)`,
  `expand_properties(true)`) or Python `cmsis-svd`, printing only what you need.

## Procedure

1. **Pick the SVD.** simbridge presets carry one (`svd="nrf52840"`, `"k210"`,
   ...); otherwise a path. Use the SVD for the exact part (an F407 SVD is not an
   F429 SVD).
2. **Find before guessing.** `svd(query="TXE|RXNE")` tells you which
   peripheral/register/bit holds a flag. `svd(peripheral="USART2")` gives the
   register map; `svd(peripheral=..., register=...)` gives every field with
   bits, access, description and enum values. Do not write a bit position from
   memory when you can look it up.
3. **Write the code** with address = peripheral base + register offset from the
   tool, and name the SVD register/field in a comment
   (`// USART2.ISR.TXE, bit 7`). Prefer a PAC generated from the same SVD
   (svd2rust) when the project already uses one.
4. **Check enabling prerequisites the SVD doesn't state:** the peripheral's
   clock-enable bit (RCC/CLOCK), power domain, pin routing (`PSEL`), and enable
   ordering. Search for them (`svd(query="USART2EN")`) but know you have to look.
5. **Verify in simulation.** `regs(peripheral, register="SR")` returns decoded
   fields; `peripheral_trace(peripheral, decode=true)` shows the actual
   read/write sequence the firmware performed, field by field. Compare against
   what you intended.
6. **Decode values you see anywhere** (logs, traces, a debugger) with
   `svd(peripheral, register, value="0x...")` instead of mental bit-twiddling.

## What the field metadata means for code

| SVD says | Implication |
|---|---|
| `access: ro` | Writes are ignored or harmful; don't include in RMW masks. |
| `access: wo` | Reading returns garbage; never RMW, track the value yourself. |
| `access: w1` / `rw1` | Writable once after reset (lock bits). |
| `readAction: clear` (or `read_side_effect`) | Reading changes state (FIFOs, clear-on-read flags). Don't poll it in debug code or read it "just to look". |
| `modifiedWriteValues: oneToClear` | Write 1 to clear that flag and 0 elsewhere; a read-modify-write can clear flags you didn't mean to. |
| `values` vs `write_values` | Enums with `usage=write` describe what to write, not what you'll read back. |
| `reset` | The expected power-on value, useful for spotting a wrong SVD or an unexpected init. |

## Known pitfalls

- **Names differ between tools.** Renode's `uart`/`gpioPortA` are the SVD's
  `UARTHS`/`GPIOA`. Match peripherals by base address, not name.
- **Same address, several descriptions.** Nordic's `UART0` and `UARTE0` share
  `0x40002000`; STM32 timers describe `CCMR1` twice (input/output); TrustZone
  parts list secure aliases (`SEC_RCC` at `0x5xxx_xxxx`). Pick the view that
  matches the mode the code uses.
- **Vendor SVDs contain errors.** Examples seen: ST's STM32F407 gives USART SR
  reset `0xC00000` (reference manual: `0x00C0`) and marks write-0-to-clear
  flags as plain `rw`; Nordic spells `read-writeOnce` as `read-writeonce`,
  which strict parsers reject. For STM32, the patched SVDs from `stm32-rs` fix
  many of these. When the SVD and the reference manual disagree, the manual
  wins; say which one you used.
- **The simulator model is a third opinion.** Renode models implement a subset
  of each peripheral and sometimes simplify (e.g. TXE always reads 1). A value
  that decodes oddly may be the model, the SVD or your code; check all three.
- **SVD stops at registers.** Clock trees, DMA request mapping, interrupt
  behaviour beyond the IRQ number, timing and errata come from the reference
  manual, datasheet or the board description (devicetree, Renode `.repl`).