# B3: UART0 banner on FE310

Write firmware for the `fe310` board (a SiFive FE310-G002 as on the HiFive1
Rev B; Renode platform `platforms/cpus/sifive-fe310.repl`) that, after reset:

1. Configures **UART0** for 115200 baud, 8 data bits, 1 stop bit, assuming a
   **16 MHz** UART input clock, and enables the transmitter. Use the registers
   directly (no HAL crate).
2. Prints exactly `B3 ready` followed by `\r\n`, once. The transmit FIFO is
   only 8 entries deep: never write a byte while it is full.
3. Then idles; it must print nothing else.

The crate is in this directory. Build it with:

    cargo build --release --target riscv32imac-unknown-none-elf

The ELF is `target/riscv32imac-unknown-none-elf/release/b3-firmware`.
