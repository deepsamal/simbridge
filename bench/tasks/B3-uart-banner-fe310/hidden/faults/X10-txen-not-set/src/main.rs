#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use panic_halt as _;
use riscv_rt::entry;

// FE310-G002 UART0 (manual chapter 18).
const UART0: usize = 0x1001_3000;
const TXDATA: *mut u32 = UART0 as *mut u32;
const DIV: *mut u32 = (UART0 + 0x18) as *mut u32;

fn putc(c: u8) {
    unsafe {
        while read_volatile(TXDATA) & (1 << 31) != 0 {} // txdata.full: FIFO full
        write_volatile(TXDATA, c as u32);
    }
}

#[entry]
fn main() -> ! {
    unsafe {
        // baud = f_in / (div + 1): 16 MHz / 139 = 115108 baud (-0.08%).
        write_volatile(DIV, 138);
    }
    b"B3 ready\r\n".iter().copied().for_each(putc);
    loop {
        riscv::asm::wfi();
    }
}
