#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;
// The PAC provides register definitions, the interrupt vector table and the
// `#[interrupt]` attribute, e.g. `use stm32f4::stm32f407::interrupt;`.
use stm32f4::stm32f407 as _;

#[entry]
fn main() -> ! {
    // TODO: see spec.md.
    loop {
        cortex_m::asm::wfi();
    }
}
