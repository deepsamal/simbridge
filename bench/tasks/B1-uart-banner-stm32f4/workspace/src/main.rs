#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

#[entry]
fn main() -> ! {
    // TODO: configure USART2 and print the banner (see spec.md).
    loop {
        cortex_m::asm::wfi();
    }
}
