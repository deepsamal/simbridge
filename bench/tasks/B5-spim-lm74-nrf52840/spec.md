# B5: LM74 temperature over SPI on nRF52840

Write firmware for the `nrf52840` board (an nRF52840; Renode platform
`platforms/cpus/nrf52840.repl`) with a **TI LM74** temperature sensor on SPI:
SCK = P0.19, SI/O = P0.21 (the LM74's data out, so the nRF's MISO),
CS = P0.20 (active low, driven by the firmware as a GPIO). After reset, the
firmware:

1. Configures UARTE0 for 115200 baud with TXD on P0.06 (as in B2).
2. Uses **SPIM2** (the SPI master with EasyDMA) in SPI mode 0, MSB first.
3. Once a second, starting right after reset, reads one conversion from the
   LM74 and prints `B5 <temperature>` followed by `\r\n`, in °C with two
   decimals, truncated toward zero: e.g. `B5 21.75`, `B5 -3.50`, `B5 0.06`.

Use the registers directly (no HAL crate).

To run it in simbridge, load it with the sensor attached and set its reading:

    load(elf=..., platform="nrf52840",
         extra_repl="spi2:\n    easyDMA: true\n\nlm74: Sensors.TI_LM74 @ spi2")
    stimulus(peripheral="spi2.lm74", property="Temperature", value=21.75)

The crate is in this directory. Build it with:

    cargo build --release --target thumbv7em-none-eabihf

The ELF is `target/thumbv7em-none-eabihf/release/b5-firmware`.
