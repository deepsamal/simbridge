#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

#[entry]
fn main() -> ! {
    // TODO: see spec.md.
    loop {
        cortex_m::asm::nop();
    }
}
