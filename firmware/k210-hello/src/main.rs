#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use panic_halt as _;
use riscv_rt::entry;

#[path = "../../common/hello.rs"]
mod hello;

// UARTHS from k210.svd: the SiFive UART block, same registers as the FE310's.
// Hart 1 never reaches main: riscv-rt parks every hart above _max_hart_id (0).
const UARTHS: usize = 0x3800_0000;
const TXDATA: *mut u32 = UARTHS as *mut u32;
const TXCTRL: *mut u32 = (UARTHS + 0x08) as *mut u32;

fn putc(c: u8) {
    unsafe {
        while read_volatile(TXDATA) & (1 << 31) != 0 {} // FIFO full
        write_volatile(TXDATA, c as u32);
    }
}

#[entry]
fn main() -> ! {
    unsafe { write_volatile(TXCTRL, 1) }; // txen
    hello::run("k210", putc, riscv::asm::nop)
}
