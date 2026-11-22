//! Built-in board presets: which Renode platform to load, which UART is the
//! console, which CPU to read the PC from, and which SVD describes the chip.

pub struct Preset {
    /// Name used in `load(platform=...)`, plus aliases.
    pub names: &'static [&'static str],
    /// Platform description shipped with Renode.
    pub repl: &'static str,
    /// Console UART.
    pub uart: &'static str,
    /// CPU whose PC is reported (multi-core platforms don't use `cpu`).
    pub cpu: &'static str,
    /// SVD path relative to the SVD directory.
    pub svd: &'static str,
    /// Renode peripheral -> SVD peripheral, where the SVD has several views at
    /// one address and the platform's model matches a specific one.
    pub svd_alias: &'static [(&'static str, &'static str)],
    /// Timers to clock at `timer_hz`. Renode's STM32 platforms clock every timer
    /// at a fixed 10 MHz; the chip's timers run from its reset clock until the
    /// firmware reprograms RCC, so firmware correct on hardware is only correct
    /// here at the reset clock. (Renode doesn't follow RCC changes.)
    pub timers: &'static [&'static str],
    pub timer_hz: u64,
    /// Extra `.repl` lines applied on top of `repl`, for model parameters that
    /// can only be set before the machine is built (constructor attributes):
    /// the STM32 presets put SysTick on the reset clock (Renode's platforms say
    /// 72 MHz for the F4/F103, 1 MHz for the L552), the nRF52840 on its 64 MHz
    /// core clock (the platform leaves Renode's default), the FE310 its CLINT on
    /// the RTC.
    /// simbridge writes `using "<repl>"` plus these lines to its work folder
    /// and loads that instead. Empty for none.
    pub overlay: &'static str,
}

const STM32_TIMERS_1_14: &[&str] = &[
    "timer1", "timer2", "timer3", "timer4", "timer5", "timer6", "timer7",
    "timer8", "timer9", "timer10", "timer11", "timer12", "timer13", "timer14",
];

pub const PRESETS: &[Preset] = &[
    Preset {
        names: &["stm32f4", "stm32f407"],
        repl: "platforms/cpus/stm32f4.repl",
        uart: "sysbus.usart2",
        cpu: "sysbus.cpu",
        svd: "cmsis-svd-data/data/STMicro/STM32F407.svd",
        svd_alias: &[],
        timers: STM32_TIMERS_1_14,
        timer_hz: 16_000_000,
        overlay: "nvic:\n    systickFrequency: 16000000\n",
    },
    Preset {
        names: &["fe310", "hifive1"],
        repl: "platforms/cpus/sifive-fe310.repl",
        uart: "sysbus.uart0",
        cpu: "sysbus.cpu",
        svd: "e310x/e310x/e310x.svd",
        svd_alias: &[],
        timers: &[],
        // mtime counts the 32.768 kHz RTC oscillator on the FE310 (HiFive1);
        // Renode's sifive-fe310.repl runs the CLINT at 62 MHz.
        timer_hz: 32_768,
        overlay: "clint:\n    frequency: 32768\n",
    },
    Preset {
        names: &["nrf52840"],
        repl: "platforms/cpus/nrf52840.repl",
        uart: "sysbus.uart0",
        cpu: "sysbus.cpu",
        svd: "cmsis-svd-data/data/Nordic/nrf52840.svd",
        // Renode's nrf52840.repl builds both UARTs with easyDMA: true.
        svd_alias: &[("uart0", "UARTE0"), ("uart1", "UARTE1")],
        timers: &[],
        timer_hz: 0,
        overlay: "nvic:\n    systickFrequency: 64000000\n",
    },
    Preset {
        names: &["stm32f103", "bluepill"],
        repl: "platforms/cpus/stm32f103.repl",
        uart: "sysbus.usart2",
        cpu: "sysbus.cpu",
        svd: "cmsis-svd-data/data/STMicro/STM32F103xx.svd",
        svd_alias: &[],
        timers: STM32_TIMERS_1_14,
        timer_hz: 8_000_000,
        overlay: "nvic:\n    systickFrequency: 8000000\n",
    },
    Preset {
        names: &["stm32f072"],
        repl: "platforms/cpus/stm32f072.repl",
        uart: "sysbus.usart2",
        cpu: "sysbus.cpu",
        svd: "cmsis-svd-data/data/STMicro/STM32F072x.svd",
        svd_alias: &[],
        timers: &[],
        timer_hz: 0,
        overlay: "",
    },
    Preset {
        names: &["stm32l552"],
        repl: "platforms/cpus/stm32l552.repl",
        uart: "sysbus.usart2",
        cpu: "sysbus.cpu",
        svd: "cmsis-svd-data/data/STMicro/STM32L552.svd",
        svd_alias: &[],
        timers: &["timer1", "timer2", "timer3", "timer4", "timer5", "timer6", "timer7", "timer8", "timer15", "timer16", "timer17"],
        timer_hz: 4_000_000,
        overlay: "nvic:\n    systickFrequency: 4000000\n",
    },
    Preset {
        names: &["k210", "kendryte_k210"],
        repl: "platforms/cpus/kendryte_k210.repl",
        uart: "sysbus.uart",
        cpu: "sysbus.cpu1",
        svd: "cmsis-svd-data/data/Kendryte-Community/k210.svd",
        svd_alias: &[],
        timers: &[],
        timer_hz: 0,
        overlay: "",
    },
];

pub fn find(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.names.iter().any(|n| n.eq_ignore_ascii_case(name)))
}

/// One-line list for tool descriptions and errors.
pub fn names() -> String {
    PRESETS.iter().map(|p| p.names[0]).collect::<Vec<_>>().join(", ")
}
