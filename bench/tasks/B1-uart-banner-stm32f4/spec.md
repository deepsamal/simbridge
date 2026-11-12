# B1: USART2 banner on STM32F4

Write firmware for the `stm32f4` board (an STM32F407; Renode platform
`platforms/cpus/stm32f4.repl`) that, after reset:

1. Configures **USART2** for 115200 baud, 8 data bits, no parity, 1 stop bit,
   assuming the default **16 MHz HSI** peripheral clock, and enables the
   transmitter. Use the register-level peripheral directly (no HAL crate).
2. Prints exactly `B1 ready` followed by `\r\n`, once.
3. Then idles; it must print nothing else.

The crate is in this directory. Build it with:

    cargo build --release --target thumbv7em-none-eabihf

The ELF is `target/thumbv7em-none-eabihf/release/b1-firmware`.
