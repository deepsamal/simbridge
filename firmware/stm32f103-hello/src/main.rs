#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use cortex_m_rt::entry;
use panic_halt as _;

#[path = "../../common/hello.rs"]
mod hello;

// Addresses from STM32F103xx.svd. Same USART layout as the F4 (SR/DR/BRR/CR1).
const RCC_APB1ENR: *mut u32 = 0x4002_101C as *mut u32;
const USART2: usize = 0x4000_4400;
const SR: *mut u32 = USART2 as *mut u32;
const DR: *mut u32 = (USART2 + 0x04) as *mut u32;
const BRR: *mut u32 = (USART2 + 0x08) as *mut u32;
const CR1: *mut u32 = (USART2 + 0x0C) as *mut u32;

fn putc(c: u8) {
    unsafe {
        while read_volatile(SR) & (1 << 7) == 0 {} // TXE
        write_volatile(DR, c as u32);
    }
}

#[entry]
fn main() -> ! {
    unsafe {
        write_volatile(RCC_APB1ENR, read_volatile(RCC_APB1ENR) | (1 << 17)); // USART2EN
        write_volatile(BRR, 0x45); // 115200 @ 8 MHz HSI (value irrelevant in Renode)
        write_volatile(CR1, (1 << 13) | (1 << 3)); // UE | TE
    }
    hello::run("stm32f103", putc, cortex_m::asm::nop)
}
