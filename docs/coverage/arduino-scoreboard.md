# Arduino × LabWired board matrix

_Generated 2026-08-12 19:10:12 +0200 by `validation/arduino-matrix/run_matrix.py`._

Legend: ✅ pass · 🔧 compile/build fail · 📦 toolchain missing · 🔴 boot/sim fail · 🟠 oracle miss · 🟣 unmodeled · ⏱️ timeout

| chip | L0_serial_boot | L1_serial_loop | L2_blink_serial | L3_i2c_sensor | L4_spi_sensor | L5_adc | L6_pwm | L7_timer | L8_can | notes |
|------|------|------|------|------|------|------|------|------|------|-------|
| `esp32` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | L8_can:skipped |
| `esp32c3` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | L8_can:skipped |
| `esp32s3` | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | ✅ | ✅ | ⏭️ | L5_adc:skipped; L8_can:skipped |
| `nrf52832` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | L8_can:skipped |
| `nrf52840` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | L8_can:skipped |
| `rp2040` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | L8_can:skipped |
| `stm32f103` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |  |
| `stm32f401` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | L8_can:skipped |
| `stm32f407` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | L8_can:skipped |
| `stm32g474re` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | L8_can:skipped |
| `stm32h563` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |  |
| `stm32l073` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | L8_can:skipped |
| `stm32l476` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |  |
| `stm32wb55` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | L8_can:skipped |
| `stm32wba52` | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | ✅ | ✅ | ⏭️ | L5_adc:skipped; L8_can:skipped |
| `atmega328p` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ⏭️ | L8_can:skipped |

## Summary

- Cells: **144**
- `pass`: 129
- `skipped`: 15

## Failures (detail)

### `esp32` × `L8_can` → **skipped**
No on-chip CAN model (TWAI not in matrix L8 yet)

### `esp32c3` × `L8_can` → **skipped**
No on-chip CAN model

### `esp32s3` × `L5_adc` → **skipped**
No SAR ADC model in esp32s3 chip yaml / programmatic bank yet

### `esp32s3` × `L8_can` → **skipped**
No on-chip CAN model

### `nrf52832` × `L8_can` → **skipped**
No on-chip CAN model

### `nrf52840` × `L8_can` → **skipped**
No on-chip CAN model

### `rp2040` × `L8_can` → **skipped**
No on-chip CAN model

### `stm32f401` × `L8_can` → **skipped**
No bxCAN/FDCAN in chip yaml

### `stm32f407` × `L8_can` → **skipped**
No bxCAN in chip yaml

### `stm32g474re` × `L8_can` → **skipped**
No FDCAN in chip yaml for matrix L8

### `stm32l073` × `L8_can` → **skipped**
No CAN peripheral on L0 series

### `stm32wb55` × `L8_can` → **skipped**
No CAN model in matrix path

### `stm32wba52` × `L5_adc` → **skipped**
No ADC model in stm32wba52 chip yaml yet

### `stm32wba52` × `L8_can` → **skipped**
No CAN model in matrix path

### `atmega328p` × `L8_can` → **skipped**
ATmega328P has no CAN controller

