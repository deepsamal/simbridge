#![no_std]
#![no_main]

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use cortex_m::peripheral::syst::SystClkSource;
use cortex_m_rt::{entry, exception};
use panic_halt as _;

// nRF52840 (product specification v1.7).
const P0: usize = 0x5000_0000;
const P0_OUTSET: *mut u32 = (P0 + 0x508) as *mut u32;
const P0_OUTCLR: *mut u32 = (P0 + 0x50C) as *mut u32;
const P0_DIRSET: *mut u32 = (P0 + 0x518) as *mut u32;

const UARTE0: usize = 0x4000_2000;
const SPIM2: usize = 0x4002_3000;

/// Board wiring: LM74 SCK = P0.19, SI/O = P0.21 (MISO), CS = P0.20 (active low).
const PIN_SCK: u32 = 19;
const PIN_MISO: u32 = 21;
const PIN_CS: u32 = 20;

fn reg(base: usize, off: usize) -> *mut u32 {
    (base + off) as *mut u32
}
fn rd(r: *mut u32) -> u32 {
    unsafe { read_volatile(r) }
}
fn wr(r: *mut u32, v: u32) {
    unsafe { write_volatile(r, v) }
}

/// SysTick fires 4 times a second (64 MHz / 16 M); main reads on every 4th.
static mut QUARTERS: u32 = 0;

#[exception]
fn SysTick() {
    unsafe { write_volatile(addr_of_mut!(QUARTERS), read_volatile(addr_of!(QUARTERS)) + 1) };
}

static mut TX_BUF: [u8; 32] = [0; 32];
static mut RX_BUF: [u8; 2] = [0; 2];

/// Sends one line through UARTE0 with EasyDMA (as in B2).
fn send(line: &[u8]) {
    unsafe {
        let buf = addr_of_mut!(TX_BUF) as *mut u8;
        for (i, b) in line.iter().enumerate() {
            write_volatile(buf.add(i), *b);
        }
    }
    wr(reg(UARTE0, 0x120), 0); // EVENTS_ENDTX
    wr(reg(UARTE0, 0x544), addr_of!(TX_BUF) as u32); // TXD.PTR
    wr(reg(UARTE0, 0x548), line.len() as u32); // TXD.MAXCNT
    wr(reg(UARTE0, 0x008), 1); // TASKS_STARTTX
    while rd(reg(UARTE0, 0x120)) == 0 {}
}

/// One LM74 conversion: CS low, clock in 16 bits, CS high.
fn read_lm74() -> u16 {
    wr(reg(SPIM2, 0x118), 0); // EVENTS_END
    wr(reg(SPIM2, 0x534), addr_of!(RX_BUF) as u32); // RXD.PTR
    wr(reg(SPIM2, 0x538), 2); // RXD.MAXCNT
    wr(reg(SPIM2, 0x548), 0); // TXD.MAXCNT: receive only
    wr(P0_OUTCLR, 1 << PIN_CS);
    wr(reg(SPIM2, 0x010), 1); // TASKS_START
    while rd(reg(SPIM2, 0x118)) == 0 {}
    wr(P0_OUTSET, 1 << PIN_CS);
    unsafe {
        let rx = addr_of!(RX_BUF) as *const u8;
        ((read_volatile(rx) as u16) << 8) | read_volatile(rx.add(1)) as u16
    }
}

/// `B5 <temp>` with two decimals, truncated toward zero (0.0625 °C/LSB).
fn format(raw: u16, out: &mut [u8; 32]) -> usize {
    let t = (raw as i16) >> 3;
    let hundredths = t as i32 * 625 / 100;
    let mut digits = [0u8; 12];
    let (mut n, mut v) = (0, hundredths.unsigned_abs());
    loop {
        digits[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if n == 2 {
            digits[n] = b'.';
            n += 1;
        }
        if v == 0 && n > 3 {
            break;
        }
    }
    let mut len = 0;
    for b in b"B5 " {
        out[len] = *b;
        len += 1;
    }
    if hundredths < 0 {
        out[len] = b'-';
        len += 1;
    }
    for i in (0..n).rev() {
        out[len] = digits[i];
        len += 1;
    }
    out[len] = b'\r';
    out[len + 1] = b'\n';
    len + 2
}

#[entry]
fn main() -> ! {
    // UARTE0: TXD on P0.06, 115200 baud.
    wr(reg(UARTE0, 0x50C), 6);
    wr(reg(UARTE0, 0x524), 0x01D7_E000);
    wr(reg(UARTE0, 0x500), 8);

    // CS idles high.
    wr(P0_OUTSET, 1 << PIN_CS);
    wr(P0_DIRSET, (1 << PIN_CS) | (1 << PIN_SCK));

    // SPIM2: mode 0, MSB first, 1 Mbps, no MOSI (the LM74 only talks here).
    wr(reg(SPIM2, 0x508), PIN_SCK); // PSEL.SCK
    wr(reg(SPIM2, 0x50C), 0xFFFF_FFFF); // PSEL.MOSI: disconnected
    wr(reg(SPIM2, 0x510), PIN_MISO); // PSEL.MISO
    wr(reg(SPIM2, 0x524), 0x1000_0000); // FREQUENCY = M1
    wr(reg(SPIM2, 0x554), 0); // CONFIG: MSB first, CPHA = 0, CPOL = 0
    wr(reg(SPIM2, 0x500), 7); // ENABLE: SPIM

    // SysTick from the 64 MHz core clock; 24-bit reload, so 0.25 s per wrap.
    let mut syst = cortex_m::Peripherals::take().unwrap().SYST;
    syst.set_clock_source(SystClkSource::Core);
    syst.set_reload(16_000_000 - 1);
    syst.clear_current();
    syst.enable_interrupt();
    syst.enable_counter();

    let mut line = [0u8; 32];
    let mut next = 0u32;
    loop {
        let len = format(read_lm74(), &mut line);
        send(&line[..len]);
        next += 4;
        while unsafe { read_volatile(addr_of!(QUARTERS)) } < next {
            cortex_m::asm::wfi();
        }
    }
}
