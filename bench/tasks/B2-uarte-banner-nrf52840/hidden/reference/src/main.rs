#![no_std]
#![no_main]

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use cortex_m_rt::entry;
use panic_halt as _;

// nRF52840 UARTE0 (EasyDMA), offsets from the product specification / nrf52840.svd.
const UARTE0: usize = 0x4000_2000;
const TASKS_STARTTX: *mut u32 = (UARTE0 + 0x008) as *mut u32;
const TASKS_STOPTX: *mut u32 = (UARTE0 + 0x00C) as *mut u32;
const EVENTS_ENDTX: *mut u32 = (UARTE0 + 0x120) as *mut u32;
const ENABLE: *mut u32 = (UARTE0 + 0x500) as *mut u32;
const PSEL_TXD: *mut u32 = (UARTE0 + 0x50C) as *mut u32;
const BAUDRATE: *mut u32 = (UARTE0 + 0x524) as *mut u32;
const TXD_PTR: *mut u32 = (UARTE0 + 0x544) as *mut u32;
const TXD_MAXCNT: *mut u32 = (UARTE0 + 0x548) as *mut u32;

const BANNER: &[u8] = b"B2 ready\r\n";

/// EasyDMA reads only RAM, so the banner is copied here from flash first.
static mut TX_BUF: [u8; 16] = [0; 16];

#[entry]
fn main() -> ! {
    unsafe {
        write_volatile(PSEL_TXD, 6); // P0.06, connected (bit 31 = 0)
        write_volatile(BAUDRATE, 0x01D7_E000); // Baud115200
        write_volatile(ENABLE, 8); // Enabled (UARTE)

        let buf = addr_of_mut!(TX_BUF) as *mut u8;
        for (i, b) in BANNER.iter().enumerate() {
            write_volatile(buf.add(i), *b);
        }
        write_volatile(EVENTS_ENDTX, 0);
        write_volatile(TXD_PTR, addr_of!(TX_BUF) as u32);
        write_volatile(TXD_MAXCNT, BANNER.len() as u32);
        write_volatile(TASKS_STARTTX, 1);
        while read_volatile(EVENTS_ENDTX) == 0 {}
        write_volatile(TASKS_STOPTX, 1);
    }
    loop {
        cortex_m::asm::wfi();
    }
}
