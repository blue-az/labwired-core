# External Components (Feather-STM32F405)

No required external simulated components for minimal deterministic smoke.

The onboarding path uses on-chip peripherals only:
1. RCC
2. GPIO
3. USART2
4. SysTick

## Adding an external device (I²C / SPI sensor, EEPROM, etc.)

See [`examples/demo-blinky/`](../demo-blinky/README.md) — that example is
the canonical reference for the `external_devices` attach pattern (TMP102
on I²C1, STM32F103). The same `connection:` / `type:` / `config:` shape
works on any chip whose corresponding bus is modeled.

Before copying, check that the bus you need is actually modeled for F405:
see [`docs/boards/stm32f405.md`](../../docs/boards/stm32f405.md). The F405
descriptor declares the F4-profile shared IP (RCC/GPIO/USART/I2C1/SPI1/
ADC1/TIM1-2/DMA1-2/EXTI/RTC/IWDG) — instance coverage beyond the F401 subset
(I2C2/SPI2/USART3/TIM3-5) is a declared follow-up, not yet present.
