# Tier-1 Validation Matrix

Every cell links the CI run that produced it; no link → `·` unrecorded.

**Confidence tier:** ✅ means *sim-consistent* — the check passed against
the simulator's peripheral models on real firmware. Silicon-anchored
verification (hardware-in-the-loop capture replay) is a separate tier
that arrives with the HIL workstream; no cell currently claims it.

| chip | clock | gpio | uart | timer | dma | irq | adc | i2c | pwm | rmt | rtc | spi | wdt |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| ESP32 (Xtensa LX6) | · | · | · | · | — | · | — | — | — | · | — | — | — |
| ESP32-C3 (RISC-V) | — | · | · | · | — | · | — | — | — | · | — | — | — |
| ESP32-S3 (Xtensa LX7) | · | · | · | · | · | · | — | · | · | · | — | — | — |
| nRF52832 | — | — | · | — | — | — | — | — | — | · | — | — | — |
| nRF52840 | — | · | · | — | — | — | — | — | — | · | — | · | — |
| RP2040 | — | — | · | — | — | — | — | — | — | · | — | — | — |
| STM32F103C8 | · | · | · | · | · | — | · | · | — | · | · | · | · |
| STM32F401RE | · | · | · | — | — | — | — | · | — | · | — | — | — |
| STM32F407VG | · | · | · | — | — | — | — | · | — | · | — | — | — |
| STM32G474RE | · | · | · | — | — | — | — | — | — | · | — | — | — |
| STM32H563 | · | · | · | — | · | — | — | — | — | · | — | — | — |
| STM32L073RZ | · | · | · | · | · | · | · | · | — | · | · | · | · |
| STM32L476RG | · | · | · | · | · | · | · | · | — | · | · | · | · |
| STM32WB55 | · | · | · | — | — | — | — | — | — | · | — | — | — |
| STM32WBA52 | · | · | · | — | — | — | — | — | — | · | — | — | — |
