#![no_std]
#![no_main]

use core::ptr::{addr_of, read_volatile, write_volatile};
use cortex_m_rt::entry;
use panic_halt as _;

#[path = "../../common/hello.rs"]
mod hello;

// UARTE0 (EasyDMA). Renode's nrf52840.repl builds uart0 with `easyDMA: true`,
// which drops the legacy UART's TXD register (0x51C), so only the DMA path
// transmits. UART0 and UARTE0 share this base; offsets from nrf52840.svd.
const UARTE0: usize = 0x4000_2000;
const TASKS_STARTTX: *mut u32 = (UARTE0 + 0x008) as *mut u32;
const EVENTS_ENDTX: *mut u32 = (UARTE0 + 0x120) as *mut u32;
const ENABLE: *mut u32 = (UARTE0 + 0x500) as *mut u32;
const PSEL_TXD: *mut u32 = (UARTE0 + 0x50C) as *mut u32;
const TXD_PTR: *mut u32 = (UARTE0 + 0x544) as *mut u32;
const TXD_MAXCNT: *mut u32 = (UARTE0 + 0x548) as *mut u32;

/// EasyDMA can only read RAM, so each byte goes through this buffer.
static mut TX_BUF: [u8; 1] = [0];

fn putc(c: u8) {
    unsafe {
        write_volatile(addr_of!(TX_BUF) as *mut u8, c);
        write_volatile(EVENTS_ENDTX, 0);
        write_volatile(TXD_PTR, addr_of!(TX_BUF) as u32);
        write_volatile(TXD_MAXCNT, 1);
        write_volatile(TASKS_STARTTX, 1);
        while read_volatile(EVENTS_ENDTX) == 0 {}
    }
}

#[entry]
fn main() -> ! {
    unsafe {
        write_volatile(PSEL_TXD, 6); // P0.06, the TX pin on the nRF52840-DK
        write_volatile(ENABLE, 8); // ENABLE = Enabled (UARTE)
    }
    hello::run("nrf52840", putc, cortex_m::asm::nop)
}
