#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};
use cortex_m_rt::entry;
use panic_halt as _;

#[path = "../../common/hello.rs"]
mod hello;

// Addresses from STM32F072x.svd (RCC_APB1ENR). Newer USART layout: CR1 at 0x00, ISR at 0x1C, TDR at 0x28.
const RCC_APB1ENR: *mut u32 = 0x4002_101C as *mut u32;
const USART2: usize = 0x4000_4400;
const CR1: *mut u32 = USART2 as *mut u32;
const BRR: *mut u32 = (USART2 + 0x0C) as *mut u32;
const ISR: *mut u32 = (USART2 + 0x1C) as *mut u32;
const TDR: *mut u32 = (USART2 + 0x28) as *mut u32;

fn putc(c: u8) {
    unsafe {
        while read_volatile(ISR) & (1 << 7) == 0 {} // TXE
        write_volatile(TDR, c as u32);
    }
}

#[entry]
fn main() -> ! {
    unsafe {
        write_volatile(RCC_APB1ENR, read_volatile(RCC_APB1ENR) | (1 << 17)); // USART2EN
        write_volatile(BRR, 0x45); // value irrelevant in Renode
        write_volatile(CR1, (1 << 3) | 1); // TE | UE
    }
    hello::run("stm32f072", putc, cortex_m::asm::nop)
}
