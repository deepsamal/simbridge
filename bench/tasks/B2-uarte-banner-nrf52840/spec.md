# B2: UARTE0 banner on nRF52840

Write firmware for the `nrf52840` board (an nRF52840; Renode platform
`platforms/cpus/nrf52840.repl`) that, after reset:

1. Configures **UARTE0** (the UART with EasyDMA) for 115200 baud with TXD on
   pin **P0.06**, and enables it. Use the registers directly (no HAL crate).
2. Sends exactly `B2 ready` followed by `\r\n`, once, through **EasyDMA**.
3. Then idles; it must print nothing else.

The crate is in this directory. Build it with:

    cargo build --release --target thumbv7em-none-eabihf

The ELF is `target/thumbv7em-none-eabihf/release/b2-firmware`.
