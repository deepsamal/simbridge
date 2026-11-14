# T1: TIM2 1 kHz tick on STM32F4

Write firmware for the `stm32f4` board (an STM32F407; Renode platform
`platforms/cpus/stm32f4.repl`) that, after reset:

1. Configures **USART2** for 115200 baud 8N1 from the default **16 MHz HSI**
   clock (as in B1) and prints `T1 start` followed by `\r\n`.
2. Configures **TIM2** to raise its **update interrupt at exactly 1 kHz**. Keep
   the reset clock configuration: TIM2 is clocked at 16 MHz.
3. In the TIM2 interrupt handler, increments a millisecond counter declared as

       #[no_mangle]
       static mut TICKS: u32 = 0;

   The counter must be advanced by the interrupt, not by polling the timer.
4. Every 1000 ticks, prints `T1 count=<ticks>` followed by `\r\n`, i.e.
   `T1 count=1000`, `T1 count=2000`, ... one virtual second apart.

Use the registers directly; the `stm32f4` peripheral access crate (PAC) is a
dependency for its interrupt vector table and register definitions. No HAL.

The crate is in this directory. Build it with:

    cargo build --release --target thumbv7em-none-eabihf

The ELF is `target/thumbv7em-none-eabihf/release/t1-firmware`.
