#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use cortex_m_rt::entry;
use panic_halt as _;

// STM32F407 (RM0090). Reset clocks: 16 MHz HSI.
const RCC_APB1ENR: *mut u32 = 0x4002_3840 as *mut u32;
const USART2: usize = 0x4000_4400;
const USART2_SR: *const u32 = USART2 as *const u32;
const USART2_DR: *mut u32 = (USART2 + 0x04) as *mut u32;
const USART2_BRR: *mut u32 = (USART2 + 0x08) as *mut u32;
const USART2_CR1: *mut u32 = (USART2 + 0x0C) as *mut u32;

const RXNE: u32 = 1 << 5;
const TXE: u32 = 1 << 7;
/// Longest line we handle.
const LINE_MAX: usize = 64;

fn putc(c: u8) {
    unsafe {
        while read_volatile(USART2_SR) & TXE == 0 {}
        write_volatile(USART2_DR, c as u32);
    }
}

fn getc() -> u8 {
    unsafe {
        while read_volatile(USART2_SR) & RXNE == 0 {}
        read_volatile(USART2_DR) as u8
    }
}

#[entry]
fn main() -> ! {
    unsafe {
        write_volatile(RCC_APB1ENR, read_volatile(RCC_APB1ENR) | (1 << 17)); // USART2EN
        write_volatile(USART2_BRR, 0x8B); // 115200 from 16 MHz
        write_volatile(USART2_CR1, (1 << 13) | (1 << 3) | (1 << 2)); // UE, TE, RE
    }
    b"B6 ready\r\n".iter().copied().for_each(putc);

    let mut line = [0u8; LINE_MAX];
    let mut len = 0;
    loop {
        match getc() {
            b'\r' => {}
            b'\n' => {
                line[..len].iter().for_each(|&c| putc(c.to_ascii_uppercase()));
                putc(b'\r');
                putc(b'\n');
                len = 0;
            }
            c if len < LINE_MAX => {
                line[len] = c;
                len += 1;
            }
            _ => {} // longer than LINE_MAX: drop the excess
        }
    }
}
