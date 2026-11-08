#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use panic_halt as _;
use riscv_rt::entry;

const UART0: usize = 0x1001_3000;
const TXDATA: *mut u32 = UART0 as *mut u32;
const TXCTRL: *mut u32 = (UART0 + 0x08) as *mut u32;

#[no_mangle]
static mut TICKS: u32 = 0;

fn putc(c: u8) {
    unsafe {
        while read_volatile(TXDATA) & (1 << 31) != 0 {} // FIFO full
        write_volatile(TXDATA, c as u32);
    }
}

fn puts(s: &str) {
    s.bytes().for_each(putc);
}

fn put_u32(mut n: u32) {
    let mut buf = [0u8; 10];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 { break; }
    }
    buf[i..].iter().copied().for_each(putc);
}

#[entry]
fn main() -> ! {
    unsafe { write_volatile(TXCTRL, 1) }; // txen
    puts("hello from fe310 (Rust no_std)\r\n");
    loop {
        let t = unsafe {
            TICKS += 1;
            TICKS
        };
        puts("tick ");
        put_u32(t);
        puts("\r\n");
        for _ in 0..200_000 {
            riscv::asm::nop();
        }
    }
}
