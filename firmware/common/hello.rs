//! Shared by the newer hello firmwares (`#[path]`-included): print a banner, then
//! "tick N" forever, keeping the count in `TICKS` so mem()/snapshot/run_until
//! tests have something in RAM to observe. Each board supplies only `putc`.

/// Counter in RAM (exported unmangled so simbridge can resolve the symbol).
#[no_mangle]
pub static mut TICKS: u32 = 0;

fn puts(putc: fn(u8), s: &str) {
    s.bytes().for_each(putc);
}

fn put_u32(putc: fn(u8), mut n: u32) {
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
    buf[i..].iter().copied().for_each(putc);
}

pub fn run(board: &str, putc: fn(u8), nop: fn()) -> ! {
    puts(putc, "hello from ");
    puts(putc, board);
    puts(putc, " (Rust no_std)\r\n");
    loop {
        let t = unsafe {
            TICKS += 1;
            TICKS
        };
        puts(putc, "tick ");
        put_u32(putc, t);
        puts(putc, "\r\n");
        for _ in 0..200_000 {
            nop();
        }
    }
}
