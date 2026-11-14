#![no_std]
#![no_main]

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use cortex_m::peripheral::NVIC;
use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f407::{interrupt, Interrupt};

// STM32F407 (RM0090). Clocks at reset: 16 MHz HSI, APB1 prescaler 1, so TIM2 counts at 16 MHz.
const RCC_APB1ENR: *mut u32 = 0x4002_3840 as *mut u32;
const USART2: usize = 0x4000_4400;
const USART2_SR: *const u32 = USART2 as *const u32;
const USART2_DR: *mut u32 = (USART2 + 0x04) as *mut u32;
const USART2_BRR: *mut u32 = (USART2 + 0x08) as *mut u32;
const USART2_CR1: *mut u32 = (USART2 + 0x0C) as *mut u32;

const TIM2: usize = 0x4000_0000;
const TIM2_CR1: *mut u32 = TIM2 as *mut u32;
const TIM2_DIER: *mut u32 = (TIM2 + 0x0C) as *mut u32;
const TIM2_SR: *mut u32 = (TIM2 + 0x10) as *mut u32;
const TIM2_EGR: *mut u32 = (TIM2 + 0x14) as *mut u32;
const TIM2_PSC: *mut u32 = (TIM2 + 0x28) as *mut u32;
const TIM2_ARR: *mut u32 = (TIM2 + 0x2C) as *mut u32;

/// Milliseconds since TIM2 started; written only by the TIM2 interrupt.
#[no_mangle]
static mut TICKS: u32 = 0;

#[interrupt]
fn TIM2() {
    unsafe {
        write_volatile(TIM2_SR, 0); // clear UIF, or the interrupt fires again at once
        write_volatile(addr_of_mut!(TICKS), read_volatile(addr_of!(TICKS)) + 1);
    }
}

fn ticks() -> u32 {
    unsafe { TICKS }
}

fn putc(c: u8) {
    unsafe {
        while read_volatile(USART2_SR) & (1 << 7) == 0 {} // TXE
        write_volatile(USART2_DR, c as u32);
    }
}

fn puts(s: &[u8]) {
    s.iter().copied().for_each(putc);
}

fn put_u32(mut n: u32) {
    let mut buf = [0u8; 10];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    puts(&buf[i..]);
}

#[entry]
fn main() -> ! {
    unsafe {
        // USART2 at 115200 8N1 from 16 MHz, as in B1.
        write_volatile(RCC_APB1ENR, read_volatile(RCC_APB1ENR) | (1 << 17) | (1 << 0)); // USART2EN, TIM2EN
        write_volatile(USART2_BRR, 0x8B);
        write_volatile(USART2_CR1, (1 << 13) | (1 << 3)); // UE, TE
    }
    puts(b"T1 start\r\n");
    unsafe {
        // 16 MHz / (PSC + 1) = 1 MHz; 1 MHz / (ARR + 1) = 1 kHz update rate.
        write_volatile(TIM2_PSC, 15);
        write_volatile(TIM2_ARR, 999);
        write_volatile(TIM2_EGR, 1); // UG: load PSC now, not at the first update
        write_volatile(TIM2_SR, 0); // UG set UIF; don't count it as a tick
        write_volatile(TIM2_DIER, 1); // UIE
        NVIC::unmask(Interrupt::TIM2);
        write_volatile(TIM2_CR1, 1); // CEN
    }
    let mut next = 1000;
    loop {
        while ticks() < next {}
        puts(b"T1 count=");
        put_u32(next);
        puts(b"\r\n");
        next += 1000;
    }
}
