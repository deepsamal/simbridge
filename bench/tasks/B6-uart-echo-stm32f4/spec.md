# B6: upper-case UART echo on STM32F4

Write firmware for the `stm32f4` board (an STM32F407; Renode platform
`platforms/cpus/stm32f4.repl`) that, after reset:

1. Configures USART2 for 115200 baud 8N1 from the default 16 MHz HSI (as in
   B1), with both transmitter and receiver enabled.
2. Prints `B6 ready` followed by `\r\n`, once.
3. Then reads lines from USART2. A line ends with `\n`; a `\r` right before it
   is not part of the line. For every line received, prints the line with
   ASCII letters `a`-`z` converted to upper case, followed by `\r\n`. An empty
   line prints just `\r\n`. Lines are at most 128 bytes long, and the host may
   send many lines back to back without waiting for the replies.

Use the registers directly (no HAL crate). To send text in simbridge, use
`uart_write(text="hello\r\n")`, then run the machine.

The crate is in this directory. Build it with:

    cargo build --release --target thumbv7em-none-eabihf

The ELF is `target/thumbv7em-none-eabihf/release/b6-firmware`.
