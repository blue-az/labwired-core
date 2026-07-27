# Why `rec_tick=1` on each WASM family (walk-forcer inventory)

**Date:** 2026-07-27  
**Commit baseline:** `a7ebbe5e` (branch `wasm-realtime-tick-inventory`)  
**Host note:** native `cargo test` inventory (not browser); bus construction matches production `SystemBus::from_config` (+ `configure_cortex_m` on Cortex-M families).

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

| Family | System | `legacy_walk_disabled` | `flash_models_ops` | iolink | forcers | `max_safe` |
|--------|--------|------------------------|--------------------|--------|---------|------------|
| **stm32f103** | `examples/ssd1306-hello-lab` | true | false | false | **0** | **512** |
| **esp32c3** | `configs/systems/esp32c3-devkit.yaml` | true | false | false | **0** | **512** |
| **esp32s3** | `configs/systems/esp32s3-zero.yaml` | true | false | false | **0** | **512** |
| **stm32h563** | `configs/systems/nucleo-h563zi-demo.yaml` | false | **true** | false | **4** | **1** |
| **rp2040** | `configs/systems/rp2040-pico.yaml` | false | false | false | **8** | **1** |
| **nrf52840** | `configs/systems/nrf52840-dk.yaml` | false | false | false | **46** | **1** |

Notes:

- **C3 / F103** are regression-green (asserted in the inventory test).
- **S3 is already walk-free on this branch** (`max_safe=512`, empty forcers).
  Earlier shipped-wasm “s3 → 1” no longer holds at this revision for the
  `esp32s3-zero` peripheral set. Keep S3 in the inventory as a guard; no
  forcer migration is required for `rec_tick` on this system yaml.
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

## esp32s3 — already 512 (on this branch)

- Chip: `configs/chips/esp32s3.yaml`
- System: `configs/systems/esp32s3-zero.yaml`
- `max_safe_tick_interval`: **512**
- `legacy_walk_disabled`: true · `flash_models_ops`: false · iolink: false
- **Forcers:** _(none)_

### Full peripheral status

| name | role | `needs_legacy_walk` | `uses_scheduler` |
|------|------|---------------------|------------------|
| uart0 | scheduler | true | true |
| gpio | inert | false | false |
| timg0 | inert | false | false |
| interrupt_core0 | inert | false | false |
| systimer | scheduler | false | true |
| system | inert | false | false |
| gdma | scheduler | false | true |
| mcpwm0 | inert | false | false |
| i2c0 | scheduler | false | true |
| rmt | inert | false | false |

> If a **heavier** S3 system yaml (e.g. OLED / fullchip) reintroduces forcers,
> re-run the inventory against that manifest before claiming browser `rec_tick`.

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

## nrf52840 — max_safe=1 (46 forcers)

- Chip: `configs/chips/nrf52840.yaml`
- System: `configs/systems/nrf52840-dk.yaml` + `configure_cortex_m`
- `max_safe_tick_interval`: **1**
- `legacy_walk_disabled`: **false**
- `flash_models_ops`: false · iolink: false · hcsr04: none

### Walk-forcers (46)

All with `needs_legacy_walk=true`, `uses_scheduler=false`:

```text
uart0, i2c0, uart1, rtc0, rtc1, rtc2, wdt, ppi, clock, gpiote, twi1,
timer0, timer1, timer2, timer3, timer4, i2s, pdm, radio, rng, ecb, temp,
saadc, pwm0, pwm1, pwm2, pwm3, qspi, nfct, ficr, uicr, nvmc, comp, qdec,
egu0, egu1, egu2, egu3, egu4, egu5, mwu, aar, usbd, usbregulator, acl,
cryptocell
```

### Non-forcers on this bus

| name | role | `needs_legacy_walk` | `uses_scheduler` |
|------|------|---------------------|------------------|
| gpio0, gpio1 | inert | false | false |
| spi2 | scheduler | true | true |
| scb, dwt | scheduler | true | true |
| nvic | inert | false | false |

> Many “forcers” may be **default-true** stubs that never do walk work. Class-A
> inert sweeps (`needs_legacy_walk → false`) are the cheapest path for those
> (same pattern as C3 / F103 campaigns). Do not migrate without a real
> walk-vs-scheduler differential when the model has non-trivial `tick()`.

---

## Mapping to monorepo plan (PR-B–E)

This inventory **unblocks** the remaining WASM real-time PRs:

| PR | Likely family focus | Primary blockers from this inventory |
|----|---------------------|--------------------------------------|
| PR-B | stm32h563 | `gpdma1`, `fdcan1`, `rtc`, `pwr` + **`flash_models_ops` policy** |
| PR-C | rp2040 | `dma`, `pio0`, `timer`, `spi0`, `i2c0`, `sio`, `xip_ssi`, `usbctrl` |
| PR-D | nrf52840 | 46-name forcer list (inert-sweep first, then real walkers) |
| PR-E | residual / S3 guard | S3 already 512 on `esp32s3-zero`; verify heavier systems if needed |

No code changes to `max_safe_tick_interval` or walk migrations in this PR
(PR-A = inventory only).
