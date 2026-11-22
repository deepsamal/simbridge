# B4: TMP108 temperature over I²C on STM32F4

Write firmware for the `stm32f4` board (an STM32F407; Renode platform
`platforms/cpus/stm32f4.repl`) with a **TI TMP108** temperature sensor on
**I2C1** (SCL = PB6, SDA = PB7, alternate function 4, external pull-ups). The
sensor's ADD0 pin is tied to GND. After reset, the firmware:

1. Configures USART2 for 115200 baud from the default 16 MHz HSI (as in B1).
2. Configures I2C1 for standard mode (100 kHz) from the 16 MHz APB1 clock.
3. Once a second, starting right after reset, reads the TMP108 temperature
   register and prints `B4 <temperature>` followed by `\r\n`, in °C with two
   decimals, truncated toward zero: e.g. `B4 21.00`, `B4 -3.50`, `B4 0.06`.
4. If the sensor doesn't acknowledge its address, prints `B4 sensor not found`
   followed by `\r\n` instead, and tries again a second later.

Use the registers directly (no HAL crate).

To run it in simbridge, load it with the sensor attached and set its reading:

    load(elf=..., platform="stm32f4",
         extra_repl="i2c1:\n    I2CReadCount: 2\n\ntmp108: Sensors.TMP108 @ i2c1 0x48")
    stimulus(peripheral="i2c1.tmp108", property="Temperature", value=21)

(The simulated TMP108 reports whole degrees only. `I2CReadCount: 2` makes
Renode's I²C model fetch both bytes of a register read from the sensor.)

The crate is in this directory. Build it with:

    cargo build --release --target thumbv7em-none-eabihf

The ELF is `target/thumbv7em-none-eabihf/release/b4-firmware`.
