# External Components (NUCLEO-F767ZI)

No required external simulated components for minimal deterministic smoke.

The onboarding path uses on-chip peripherals only: RCC, GPIO, USART, SysTick.

## Adding an external device

See [`examples/demo-blinky/`](../demo-blinky/README.md) for the
`external_devices` attach pattern. The F767 I2C instances run the modern v2
model (`stm32l4` profile), so I2C sensors that attach on L4 attach here;
check [`docs/boards/stm32f767.md`](../../docs/boards/stm32f767.md) for the
modeled-peripheral boundary (no Ethernet/LTDC/CAN/USB/SAI/QSPI).
