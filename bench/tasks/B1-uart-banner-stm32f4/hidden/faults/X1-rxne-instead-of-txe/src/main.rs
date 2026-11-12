#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use cortex_m_rt::entry;
use panic_halt as _;

// STM32F407 (RM0090): RCC base 0x4002_3800, USART2 base 0x4000_4400.
const RCC_APB1ENR: *mut u32 = 0x4002_3840 as *mut u32;
const USART2: usize = 0x4000_4400;
const SR: *const u32 = USART2 as *const u32;
const DR: *mut u32 = (USART2 + 0x04) as *mut u32;
const BRR: *mut u32 = (USART2 + 0x08) as *mut u32;
const CR1: *mut u32 = (USART2 + 0x0C) as *mut u32;

const SR_TXE: u32 = 1 << 5;
const CR1_UE: u32 = 1 << 13;
const CR1_TE: u32 = 1 << 3;

fn putc(c: u8) {
    unsafe {
        while read_volatile(SR) & SR_TXE == 0 {}
        write_volatile(DR, c as u32);
    }
}

#[entry]
fn main() -> ! {
    unsafe {
        write_volatile(RCC_APB1ENR, read_volatile(RCC_APB1ENR) | (1 << 17)); // USART2EN
        // 16 MHz / (16 * 115200) = 8.68 -> mantissa 8, fraction round(0.68 * 16) = 11 -> 0x8B
        write_volatile(BRR, 0x8B);
        write_volatile(CR1, CR1_UE | CR1_TE); // 8N1 is the reset default
    }
    b"B1 ready\r\n".iter().copied().for_each(putc);
    loop {
        cortex_m::asm::wfi();
    }
}
