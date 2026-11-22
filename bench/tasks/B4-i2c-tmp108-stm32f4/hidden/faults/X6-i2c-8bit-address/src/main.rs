#![no_std]
#![no_main]

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use cortex_m::peripheral::syst::SystClkSource;
use cortex_m_rt::{entry, exception};
use panic_halt as _;

// STM32F407 (RM0090). Reset clocks: 16 MHz HSI, APB1 = 16 MHz.
const RCC_AHB1ENR: *mut u32 = 0x4002_3830 as *mut u32;
const RCC_APB1ENR: *mut u32 = 0x4002_3840 as *mut u32;

const GPIOB: usize = 0x4002_0400;
const GPIOB_MODER: *mut u32 = GPIOB as *mut u32;
const GPIOB_OTYPER: *mut u32 = (GPIOB + 0x04) as *mut u32;
const GPIOB_PUPDR: *mut u32 = (GPIOB + 0x0C) as *mut u32;
const GPIOB_AFRL: *mut u32 = (GPIOB + 0x20) as *mut u32;

const USART2: usize = 0x4000_4400;
const USART2_SR: *const u32 = USART2 as *const u32;
const USART2_DR: *mut u32 = (USART2 + 0x04) as *mut u32;
const USART2_BRR: *mut u32 = (USART2 + 0x08) as *mut u32;
const USART2_CR1: *mut u32 = (USART2 + 0x0C) as *mut u32;

const I2C1: usize = 0x4000_5400;
const I2C1_CR1: *mut u32 = I2C1 as *mut u32;
const I2C1_CR2: *mut u32 = (I2C1 + 0x04) as *mut u32;
const I2C1_DR: *mut u32 = (I2C1 + 0x10) as *mut u32;
const I2C1_SR1: *mut u32 = (I2C1 + 0x14) as *mut u32;
const I2C1_SR2: *const u32 = (I2C1 + 0x18) as *const u32;
const I2C1_CCR: *mut u32 = (I2C1 + 0x1C) as *mut u32;
const I2C1_TRISE: *mut u32 = (I2C1 + 0x20) as *mut u32;

/// Set by SysTick once a second.
static mut SECOND: bool = false;

#[exception]
fn SysTick() {
    unsafe { write_volatile(addr_of_mut!(SECOND), true) };
}

/// TMP108 7-bit address with ADD0 tied to GND.
const TMP108_ADDR: u8 = 0x90;

// I2C SR1 / CR1 bits
const SB: u32 = 1 << 0;
const ADDR: u32 = 1 << 1;
const BTF: u32 = 1 << 2;
const TXE: u32 = 1 << 7;
const AF: u32 = 1 << 10;
const START: u32 = 1 << 8;
const STOP: u32 = 1 << 9;
const ACK: u32 = 1 << 10;
const POS: u32 = 1 << 11;

fn rd(r: *const u32) -> u32 {
    unsafe { read_volatile(r) }
}
fn wr(r: *mut u32, v: u32) {
    unsafe { write_volatile(r, v) }
}
fn set(r: *mut u32, bits: u32) {
    wr(r, rd(r) | bits)
}
fn clr(r: *mut u32, bits: u32) {
    wr(r, rd(r) & !bits)
}

fn putc(c: u8) {
    while rd(USART2_SR) & (1 << 7) == 0 {}
    wr(USART2_DR, c as u32);
}
fn puts(s: &[u8]) {
    s.iter().copied().for_each(putc);
}

/// Waits for any of `bits` in SR1; gives up on NACK (AF) or after a while.
fn wait(bits: u32) -> bool {
    for _ in 0..100_000 {
        let sr1 = rd(I2C1_SR1);
        if sr1 & AF != 0 {
            return false;
        }
        if sr1 & bits != 0 {
            return true;
        }
    }
    false
}

fn abort() -> Option<i16> {
    set(I2C1_CR1, STOP);
    clr(I2C1_SR1, AF);
    None
}

/// Reads the TMP108 temperature register (pointer 0x00): 12 bits, left-justified.
fn read_temp_raw() -> Option<i16> {
    // Write the pointer byte.
    set(I2C1_CR1, START);
    if !wait(SB) {
        return abort();
    }
    wr(I2C1_DR, (TMP108_ADDR as u32) << 1);
    if !wait(ADDR) {
        return abort();
    }
    let _ = (rd(I2C1_SR1), rd(I2C1_SR2)); // clear ADDR
    wr(I2C1_DR, 0x00);
    if !wait(TXE) {
        return abort();
    }
    // Repeated start, read two bytes (RM0090 27.3.3, case N = 2).
    set(I2C1_CR1, START);
    if !wait(SB) {
        return abort();
    }
    wr(I2C1_DR, ((TMP108_ADDR as u32) << 1) | 1);
    set(I2C1_CR1, ACK | POS);
    if !wait(ADDR) {
        return abort();
    }
    clr(I2C1_CR1, ACK);
    let _ = (rd(I2C1_SR1), rd(I2C1_SR2)); // clear ADDR
    if !wait(BTF) {
        return abort();
    }
    set(I2C1_CR1, STOP);
    let msb = rd(I2C1_DR) as u8;
    let lsb = rd(I2C1_DR) as u8;
    clr(I2C1_CR1, POS);
    Some((((msb as u16) << 8) | lsb as u16) as i16 >> 4)
}

/// Prints `B4 <temp>` with two decimals, truncated toward zero (0.0625 °C/LSB).
fn print_temp(raw: i16) {
    let hundredths = raw as i32 * 625 / 100;
    let mut buf = [0u8; 16];
    let mut n = 0;
    let mut v = hundredths.unsigned_abs();
    loop {
        buf[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if n == 2 {
            buf[n] = b'.';
            n += 1;
        }
        if v == 0 && n > 3 {
            break;
        }
    }
    if hundredths < 0 {
        buf[n] = b'-';
        n += 1;
    }
    puts(b"B4 ");
    for i in (0..n).rev() {
        putc(buf[i]);
    }
    puts(b"\r\n");
}

#[entry]
fn main() -> ! {
    set(RCC_AHB1ENR, 1 << 1); // GPIOBEN
    set(RCC_APB1ENR, (1 << 21) | (1 << 17)); // I2C1EN, USART2EN

    // PB6 = I2C1_SCL, PB7 = I2C1_SDA: alternate function 4, open-drain, pull-ups.
    wr(GPIOB_MODER, (rd(GPIOB_MODER) & !(0xF << 12)) | (0b1010 << 12));
    set(GPIOB_OTYPER, (1 << 6) | (1 << 7));
    wr(GPIOB_PUPDR, (rd(GPIOB_PUPDR) & !(0xF << 12)) | (0b0101 << 12));
    wr(GPIOB_AFRL, (rd(GPIOB_AFRL) & !(0xFF << 24)) | (0x44 << 24));

    wr(USART2_BRR, 0x8B); // 115200 from 16 MHz
    wr(USART2_CR1, (1 << 13) | (1 << 3));

    // I2C1 standard mode, 100 kHz from a 16 MHz APB1.
    wr(I2C1_CR2, 16); // FREQ = 16 MHz
    wr(I2C1_CCR, 80); // 16 MHz / (2 * 80) = 100 kHz
    wr(I2C1_TRISE, 17); // 1000 ns / 62.5 ns + 1
    wr(I2C1_CR1, 1); // PE

    // SysTick from the 16 MHz core clock: one interrupt a second.
    let mut syst = cortex_m::Peripherals::take().unwrap().SYST;
    syst.set_clock_source(SystClkSource::Core);
    syst.set_reload(16_000_000 - 1);
    syst.clear_current();
    syst.enable_interrupt();
    syst.enable_counter();

    loop {
        match read_temp_raw() {
            Some(raw) => print_temp(raw),
            None => puts(b"B4 sensor not found\r\n"),
        }
        // Sleep until the next second.
        while !unsafe { read_volatile(addr_of!(SECOND)) } {
            cortex_m::asm::wfi();
        }
        unsafe { write_volatile(addr_of_mut!(SECOND), false) };
    }
}
