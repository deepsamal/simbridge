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
}

pub const PRESETS: &[Preset] = &[
    Preset {
        names: &["stm32f4", "stm32f407"],
        repl: "platforms/cpus/stm32f4.repl",
        uart: "sysbus.usart2",
        cpu: "sysbus.cpu",
        svd: "cmsis-svd-data/data/STMicro/STM32F407.svd",
        svd_alias: &[],
    },
    Preset {
        names: &["fe310", "hifive1"],
        repl: "platforms/cpus/sifive-fe310.repl",
        uart: "sysbus.uart0",
        cpu: "sysbus.cpu",
        svd: "e310x/e310x/e310x.svd",
        svd_alias: &[],
    },
    Preset {
        names: &["nrf52840"],
        repl: "platforms/cpus/nrf52840.repl",
        uart: "sysbus.uart0",
        cpu: "sysbus.cpu",
        svd: "cmsis-svd-data/data/Nordic/nrf52840.svd",
        // Renode's nrf52840.repl builds both UARTs with easyDMA: true.
        svd_alias: &[("uart0", "UARTE0"), ("uart1", "UARTE1")],
    },
    Preset {
        names: &["stm32f103", "bluepill"],
        repl: "platforms/cpus/stm32f103.repl",
        uart: "sysbus.usart2",
        cpu: "sysbus.cpu",
        svd: "cmsis-svd-data/data/STMicro/STM32F103xx.svd",
        svd_alias: &[],
    },
    Preset {
        names: &["stm32f072"],
        repl: "platforms/cpus/stm32f072.repl",
        uart: "sysbus.usart2",
        cpu: "sysbus.cpu",
        svd: "cmsis-svd-data/data/STMicro/STM32F072x.svd",
        svd_alias: &[],
    },
    Preset {
        names: &["stm32l552"],
        repl: "platforms/cpus/stm32l552.repl",
        uart: "sysbus.usart2",
        cpu: "sysbus.cpu",
        svd: "cmsis-svd-data/data/STMicro/STM32L552.svd",
        svd_alias: &[],
    },
    Preset {
        names: &["k210", "kendryte_k210"],
        repl: "platforms/cpus/kendryte_k210.repl",
        uart: "sysbus.uart",
        cpu: "sysbus.cpu1",
        svd: "cmsis-svd-data/data/Kendryte-Community/k210.svd",
        svd_alias: &[],
    },
];

pub fn find(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.names.iter().any(|n| n.eq_ignore_ascii_case(name)))
}

/// One-line list for tool descriptions and errors.
pub fn names() -> String {
    PRESETS.iter().map(|p| p.names[0]).collect::<Vec<_>>().join(", ")
}
