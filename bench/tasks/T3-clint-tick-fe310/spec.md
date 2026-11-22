# T3: CLINT machine-timer tick on FE310

Write firmware for the `fe310` board (a SiFive FE310-G002 as on the HiFive1
Rev B; Renode platform `platforms/cpus/sifive-fe310.repl`) that, after reset:

1. Configures UART0 for 115200 baud from a 16 MHz input clock (as in B3) and
   prints `T3 start` followed by `\r\n`, once.
2. Raises the **machine-timer interrupt** from the CLINT **100 times a
   second**. `mtime` counts the **32.768 kHz** real-time clock; 32768 / 100 is
   not a whole number, so keep the long-run rate exact rather than rounding
   every period.
3. In the interrupt handler, increments a counter declared as

       #[no_mangle]
       static mut TICKS: u32 = 0;

   The counter must be advanced by the interrupt, not by polling `mtime`.
4. Otherwise idles (e.g. `wfi`); it prints nothing after the start line.

Use the registers and CSRs directly (the `riscv` crate is a dependency for CSR
access; no HAL). The crate is in this directory. Build it with:

    cargo build --release --target riscv32imac-unknown-none-elf

The ELF is `target/riscv32imac-unknown-none-elf/release/t3-firmware`.
