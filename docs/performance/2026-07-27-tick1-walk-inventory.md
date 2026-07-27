# Why `rec_tick=1` on each WASM family (walk-forcer inventory)

**Date:** 2026-07-27  
**Commit baseline:** branch `wasm-realtime-tick-inventory`  
**Host note:** native `cargo test` inventory (not browser). Cortex-M / C3 / RP2040 / nRF use production `SystemBus::from_config` (+ `configure_cortex_m` where applicable). **ESP32-S3** uses the production WASM path `configure_xtensa_esp32s3` (not chip-YAML `from_config` stubs).

## Why this exists

Shipped WASM was observed to recommend `peripheral_tick_interval = 1` on
h563 / rp2040 / nrf / s3, while c3 / f103 already reach **512**.

`SystemBus::max_safe_tick_interval` (see `crates/core/src/bus/policy.rs`) returns
`RECOMMENDED_TICK_INTERVAL` (512) only when **all** of:

1. `legacy_walk_disabled` (walk auto-deleted or hand-flagged)
2. `!has_iolink_master()`
3. `!flash_models_ops`
4. `!hcsr04_forced_legacy` (test-only override; unused on these systems)

Under `event-scheduler`, `legacy_walk_disabled` auto-derives when every
peripheral satisfies `uses_scheduler() || !needs_legacy_walk()`. The walk-forcing
set is the negation:

```text
needs_legacy_walk() && !uses_scheduler()
```

This document records that set per family so PR-B–E of the monorepo WASM
real-time plan can migrate forcers (or clear non-forcer blockers) without
guessing.

**Inventory only** — no walk migrations, no `max_safe` changes.

## How to reproduce

```bash
cargo test -p labwired-core --features event-scheduler \
  --test tick_interval_inventory -- --nocapture
```

Source: `crates/core/tests/tick_interval_inventory.rs`  
Each case builds with `walk_deleted = None` (auto-derive).

## Summary table (event-scheduler)

| Family | System / bus path | `legacy_walk_disabled` | `flash_models_ops` | iolink | forcers | `max_safe` |
|--------|-------------------|------------------------|--------------------|--------|---------|------------|
| **stm32f103** | `examples/ssd1306-hello-lab` + `configure_cortex_m` | true | false | false | **0** | **512** |
| **esp32c3** | `configs/systems/esp32c3-devkit.yaml` | true | false | false | **0** | **512** |
| **nrf52840** | `configs/systems/nrf52840-dk.yaml` + `configure_cortex_m` | true | false | false | **0** | **512** |
| **esp32s3** | `configure_xtensa_esp32s3` (WASM / production) | false | false | false | **38** | **1** |
| **stm32h563** | `configs/systems/nucleo-h563zi-demo.yaml` | false | **true** | false | **4** | **1** |
| **rp2040** | `configs/systems/rp2040-pico.yaml` | false | false | false | **8** | **1** |

Notes:

- **C3 / F103 / nRF52840** are regression-green (asserted in the inventory test
  and the PR-B gate `nrf52840_dk_is_walk_free_and_tick_512`).
- **S3 is NOT walk-free on the production path** (`max_safe=1`, **38 forcers**).
  An earlier inventory revision used `SystemBus::from_config` on
  `esp32s3.yaml` / `esp32s3-zero.yaml`, which **stubs** real S3 models
  (rmt/gdma/systimer/…) and falsely reported walk-free / `max_safe=512`.
  Production WASM (`WasmSimulator::new_from_config_xtensa_esp32s3`) and
  firmware e2e/oracle buses call `configure_xtensa_esp32s3` →
  `register_esp32s3_peripherals`; the inventory now mirrors that factory.
- **H563** is double-blocked: even if the 4 forcers migrate, `flash_models_ops`
  (H5 FLASH pending erase/bank-swap ops) still forces `max_safe=1` until that
  policy arm is relaxed or ops become batch-safe.

---

## stm32f103 — already 512

- Chip: `configs/chips/stm32f103.yaml`
- System: `examples/ssd1306-hello-lab/system.yaml` + `configure_cortex_m`
- `max_safe_tick_interval`: **512**
- `legacy_walk_disabled`: true · `flash_models_ops`: false · iolink: false
- **Forcers:** _(none)_

### Full peripheral status

| name | role | `needs_legacy_walk` | `uses_scheduler` |
|------|------|---------------------|------------------|
| rcc | inert | false | false |
| gpioa/b/c | inert | false | false |
| systick | scheduler | true | true |
| uart1/2, usart3 | scheduler | true | true |
| i2c1/2 | scheduler | false | true |
| spi1/2 | scheduler | true | true |
| afio | inert | false | false |
| exti | scheduler | false | true |
| dma1 | scheduler | false | true |
| flash_ctrl, dbgmcu, pwr, iwdg, wwdg, rtc, crc, bxcan1, usb_dev, bkp | inert | false | false |
| tim1–tim4 | scheduler | false | true |
| adc1/2 | scheduler | false | true |
| scb | scheduler | true | true |
| nvic | inert | false | false |
| dwt | scheduler | true | true |

---

## esp32c3 — already 512

- Chip: `configs/chips/esp32c3.yaml`
- System: `configs/systems/esp32c3-devkit.yaml` (plain `from_config`; no ROM inject)
- `max_safe_tick_interval`: **512**
- `legacy_walk_disabled`: true · `flash_models_ops`: false · iolink: false
- **Forcers:** _(none)_

### Full peripheral status

| name | role | `needs_legacy_walk` | `uses_scheduler` |
|------|------|---------------------|------------------|
| uart0/1 | scheduler | true | true |
| gpio | scheduler | true | true |
| timg0/1 | scheduler | true | true |
| interrupt_core0 | inert | false | false |
| system, rtc_cntl, apb_ctrl, systimer, io_mux | inert | false | false |
| i2c0 | scheduler | true | true |
| spi2 | scheduler | true | true |
| ledc | scheduler | false | true |
| rmt, spi0/1, gpio_sd, efuse, uhci0/1, bb, twai0, i2s0 | inert | false | false |
| aes, sha, rsa, ds, hmac, dma | inert | false | false |
| apb_saradc | scheduler | true | true |
| usb_device, sensitive, extmem, xts_aes, assist_debug | inert | false | false |
| radio_fe, radio_nrx, wifi_mac | inert | false | false |

---

## esp32s3 — max_safe=1 (38 forcers on production path)

- **Bus path:** `SystemBus::new()` + `configure_xtensa_esp32s3(&Esp32s3Opts::default())`
  + `recompute_walk_deletable()` (inventory auto-derive; same predicate as
  `from_config` with `walk_deleted: null`)
- **Not** `SystemBus::from_config` on chip YAML — that path stubs the coded S3
  models and previously produced a **false walk-free** inventory
- Mirrors: `WasmSimulator::new_from_config_xtensa_esp32s3`,
  `esp32s3_reset_conformance`, S3 e2e / walk-differential tests
- `max_safe_tick_interval`: **1**
- `legacy_walk_disabled`: **false**
- `flash_models_ops`: false · iolink: false · hcsr04: none

### Walk-forcers (38)

All with `needs_legacy_walk=true`, `uses_scheduler=false`:

```text
intmatrix, crosscore_ipi, core1_control, extmem, system_regs, system_regs_hi,
rtc_cntl, systimer, gpio, sens_s3, rng, sha, pcnt, ledc, timg0_s3, timg1_s3,
rmt_s3, spi2_s3, spi3_s3, sar_adc_s3, gdma, i2s0_s3, i2s1_s3, twai, aes, rsa,
hmac, ds, mcpwm0, mcpwm1, sdmmc, lcd_cam, usb_otg, i2c1, uart0_s3, uart1_s3,
uart2_s3, i2c0
```

Notable real models that the `from_config` stub path had mis-classified (or
omitted) include **systimer**, **gdma**, **rmt_s3**, **gpio**, **uart\*_s3**,
**timg\*_s3**, **i2c0/1**, **spi2/3_s3** — these force the walk on the production
bank even when YAML stubs looked scheduler/inert.

### Non-forcers on this bus (memory map / stubs)

| name | role | `needs_legacy_walk` | `uses_scheduler` |
|------|------|---------------------|------------------|
| iram, dram, rtc_slow, rtc_fast | inert | false | false |
| flash_icache, flash_dcache | inert | false | false |
| rom, drom | inert | false | false |
| spimem1 | inert | false | false |
| system (catch-all stub) | inert | false | false |
| efuse, low_mmio, mmio_rest | inert | false | false |
| usb_serial_jtag | inert | false | false |
| io_mux | inert | false | false |

**Unblock path for PR planning:** migrate the 38 forcers (Class-A inert sweep
where `tick()` is empty / default-true, else real scheduler + differential
gates — same pattern as C3). Do **not** trust chip-YAML `from_config` S3
inventory until Stage-3 factory parity lands.

---

## stm32h563 — max_safe=1 (forcers + flash_models_ops)

- Chip: `configs/chips/stm32h563.yaml`
- System: `configs/systems/nucleo-h563zi-demo.yaml` + `configure_cortex_m`
- `max_safe_tick_interval`: **1**
- `legacy_walk_disabled`: **false**
- `flash_models_ops`: **true** (H5 `flash_iface` models erase/bank-swap pending ops)
- iolink: false · hcsr04: none

### Walk-forcers (4)

| name | `needs_legacy_walk` | `uses_scheduler` |
|------|---------------------|------------------|
| **gpdma1** | true | false |
| **fdcan1** | true | false |
| **rtc** | true | false |
| **pwr** | true | false |

### Full peripheral status

| name | role | `needs_legacy_walk` | `uses_scheduler` |
|------|------|---------------------|------------------|
| rcc | inert | false | false |
| gpioa–g | inert | false | false |
| systick | scheduler | true | true |
| uart3 | scheduler | true | true |
| **gpdma1** | **FORCER** | true | false |
| **fdcan1** | **FORCER** | true | false |
| tim1_pwm, tim2, tim3, tim12, tim6 | scheduler | false | true |
| i2c1/2 | scheduler | false | true |
| uart1/2, lpuart1 | scheduler | true | true |
| wwdg, iwdg, rng, crc, lptim1 | inert | false | false |
| spi1/2/3 | scheduler | true | true |
| adc1 | scheduler | false | true |
| **rtc** | **FORCER** | true | false |
| nvic | inert | false | false |
| **pwr** | **FORCER** | true | false |
| flash_iface | inert (but sets `flash_models_ops`) | false | false |
| dbgmcu, icache | inert | false | false |
| scb, dwt | scheduler | true | true |

**Unblock path for PR planning:**

1. Migrate **gpdma1, fdcan1, rtc, pwr** off the walk (`uses_scheduler` or
   `!needs_legacy_walk` with proof).
2. Separately address **`flash_models_ops`**: today any H5 FLASH that models
   ops pins `max_safe` to 1 even on a walk-deleted bus. That is a policy /
   batching problem, not a walk-forcer.

---

## rp2040 — max_safe=1 (8 forcers)

- Chip: `configs/chips/rp2040.yaml`
- System: `configs/systems/rp2040-pico.yaml` + `configure_cortex_m`
- `LABWIRED_RP2040_BOOTROM=""` (same as other RP2040 tests; bootrom is not a forcer)
- `max_safe_tick_interval`: **1**
- `legacy_walk_disabled`: **false**
- `flash_models_ops`: false · iolink: false · hcsr04: none

### Walk-forcers (8)

| name | `needs_legacy_walk` | `uses_scheduler` |
|------|---------------------|------------------|
| **dma** | true | false |
| **pio0** | true | false |
| **timer** | true | false |
| **spi0** | true | false |
| **i2c0** | true | false |
| **sio** | true | false |
| **xip_ssi** | true | false |
| **usbctrl** | true | false |

### Full peripheral status

| name | role | `needs_legacy_walk` | `uses_scheduler` |
|------|------|---------------------|------------------|
| sysinfo | inert | false | false |
| **dma** | **FORCER** | true | false |
| **pio0** | **FORCER** | true | false |
| clk_rst | inert | false | false |
| uart0 | scheduler | true | true |
| rosc, watchdog | inert | false | false |
| **timer** | **FORCER** | true | false |
| **spi0** | **FORCER** | true | false |
| **i2c0** | **FORCER** | true | false |
| systick | scheduler | true | true |
| **sio** | **FORCER** | true | false |
| **xip_ssi** | **FORCER** | true | false |
| **usbctrl** | **FORCER** | true | false |
| tbman | inert | false | false |
| scb | scheduler | true | true |
| nvic | inert | false | false |
| dwt | scheduler | true | true |

---

## nrf52840 — already 512 (PR-B)

- Chip: `configs/chips/nrf52840.yaml`
- System: `configs/systems/nrf52840-dk.yaml` + `configure_cortex_m`
- `walk_deleted = None` (auto-derive)
- `max_safe_tick_interval`: **512**
- `legacy_walk_disabled`: **true**
- `flash_models_ops`: false · iolink: false · hcsr04: none
- **Forcers:** _(none)_

### Migration summary (46 → 0)

| Class | Models | Mechanism |
|-------|--------|-----------|
| **Class-A inert** | `ficr`, `uicr`, `nvmc`, `acl`, `cryptocell`, `mwu`, `aar`, `comp`, `qdec`, `i2s`, `pdm`, `qspi`, `nfct`, `usbd`, `usbregulator`, `ppi`, `temp`, `uarte` (uart0/1), `pwm0–3`, `saadc`, … | `needs_legacy_walk = false` (no time-driven `tick()`; EasyDMA rides `bus_tick_indices`) |
| **Class-B scheduler** | `timer0–4`, `rtc0–2`, `wdt`, `rng`, `clock`, `egu0–5`, `gpiote`, `twim`/`serial` (i2c0, twi1), `ecb`, `radio` | `uses_scheduler` + `take_scheduled_events` / `on_event` (CycleClock where counters advance) |

Featureless builds still report `max_safe=1` (honest). Gate:
`nrf52840_dk_is_walk_free_and_tick_512` in `tick_interval_inventory.rs`.

### Full peripheral status (representative)

| name | role | `needs_legacy_walk` | `uses_scheduler` |
|------|------|---------------------|------------------|
| uart0/1 | inert (EasyDMA via bus_tick) | false | false |
| i2c0, twi1 | scheduler | false | true |
| gpio0/1 | inert | false | false |
| rtc0–2, timer0–4, wdt, rng | scheduler | false | true |
| clock, egu0–5, gpiote, radio, ecb | scheduler | false | true |
| ppi, temp, saadc, pwm*, ficr, … | inert | false | false |
| spi2, scb, dwt | scheduler | true | true |
| nvic | inert | false | false |

---

## Mapping to monorepo plan (PR-B–E)

| PR | Family focus | Status / blockers |
|----|--------------|-------------------|
| **PR-B** | **nrf52840** | **DONE** — empty forcers, `max_safe=512` under `event-scheduler` |
| PR-C | rp2040 | `dma`, `pio0`, `timer`, `spi0`, `i2c0`, `sio`, `xip_ssi`, `usbctrl` |
| PR-D | stm32h563 | `gpdma1`, `fdcan1`, `rtc`, `pwr` + **`flash_models_ops` policy** |
| PR-E | esp32s3 | 38 forcers on `configure_xtensa_esp32s3` production bank (not YAML stubs) |
