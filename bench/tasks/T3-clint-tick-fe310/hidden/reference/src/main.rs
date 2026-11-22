#![no_std]
#![no_main]

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use panic_halt as _;
use riscv::register::{mie, mstatus};
use riscv_rt::entry;

// FE310-G002 CLINT (manual chapter 9). mtime counts the 32.768 kHz RTC clock.
const MTIMECMP_LO: *mut u32 = 0x0200_4000 as *mut u32;
const MTIMECMP_HI: *mut u32 = 0x0200_4004 as *mut u32;
const MTIME_LO: *const u32 = 0x0200_BFF8 as *const u32;
const MTIME_HI: *const u32 = 0x0200_BFFC as *const u32;
const MTIME_HZ: u64 = 32_768;
const TICK_HZ: u64 = 100;

const UART0: usize = 0x1001_3000;
const TXDATA: *mut u32 = UART0 as *mut u32;
const TXCTRL: *mut u32 = (UART0 + 0x08) as *mut u32;
const DIV: *mut u32 = (UART0 + 0x18) as *mut u32;

/// 10 ms ticks since start; written only by the machine-timer interrupt.
#[no_mangle]
static mut TICKS: u32 = 0;

fn mtime() -> u64 {
    loop {
        // The two halves aren't read atomically: retry if the high word moved.
        let hi = unsafe { read_volatile(MTIME_HI) };
        let lo = unsafe { read_volatile(MTIME_LO) };
        if unsafe { read_volatile(MTIME_HI) } == hi {
            return ((hi as u64) << 32) | lo as u64;
        }
    }
}

fn set_mtimecmp(t: u64) {
    unsafe {
        // Write high to max first so no spurious compare match happens mid-update.
        write_volatile(MTIMECMP_HI, u32::MAX);
        write_volatile(MTIMECMP_LO, t as u32);
        write_volatile(MTIMECMP_HI, (t >> 32) as u32);
    }
}

/// mtime value at which tick number `tick` is due.
fn cmp_for(tick: u64, start: u64) -> u64 {
    start + tick * MTIME_HZ / TICK_HZ
}

static mut START: u64 = 0;

#[export_name = "MachineTimer"]
fn machine_timer() {
    unsafe {
        let t = read_volatile(addr_of!(TICKS)) + 1;
        write_volatile(addr_of_mut!(TICKS), t);
        // Rearming mtimecmp past mtime also clears the pending interrupt.
        set_mtimecmp(cmp_for(t as u64 + 1, read_volatile(addr_of!(START))));
    }
}

fn putc(c: u8) {
    unsafe {
        while read_volatile(TXDATA) & (1 << 31) != 0 {}
        write_volatile(TXDATA, c as u32);
    }
}

#[entry]
fn main() -> ! {
    unsafe {
        write_volatile(DIV, 138);
        write_volatile(TXCTRL, 1);
    }
    b"T3 start\r\n".iter().copied().for_each(putc);
    unsafe {
        let start = mtime();
        write_volatile(addr_of_mut!(START), start);
        set_mtimecmp(cmp_for(1, start));
        mie::set_mtimer();
        mstatus::set_mie();
    }
    loop {
        riscv::asm::wfi();
    }
}
