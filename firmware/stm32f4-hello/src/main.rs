#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use cortex_m_rt::entry;
use panic_halt as _;

const RCC_APB1ENR: *mut u32 = 0x4002_3840 as *mut u32;
const USART2: usize = 0x4000_4400;
const SR: *mut u32 = USART2 as *mut u32;
const DR: *mut u32 = (USART2 + 0x04) as *mut u32;
const BRR: *mut u32 = (USART2 + 0x08) as *mut u32;
const CR1: *mut u32 = (USART2 + 0x0C) as *mut u32;

/// Counter in RAM so mem()/snapshot tests have something to observe.
#[no_mangle]
static mut TICKS: u32 = 0;

fn putc(c: u8) {
    unsafe {
        while read_volatile(SR) & (1 << 7) == 0 {} // TXE
        write_volatile(DR, c as u32);
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
    unsafe {
        write_volatile(RCC_APB1ENR, read_volatile(RCC_APB1ENR) | (1 << 17)); // USART2EN
        write_volatile(BRR, 0x8B); // 115200 @ 16 MHz (value irrelevant in Renode)
        write_volatile(CR1, (1 << 13) | (1 << 3)); // UE | TE
    }
    puts("hello from stm32f4 (Rust no_std)\r\n");
    loop {
        let t = unsafe {
            TICKS += 1;
            TICKS
        };
        puts("tick ");
        put_u32(t);
        puts("\r\n");
        for _ in 0..200_000 {
            cortex_m::asm::nop();
        }
    }
}
