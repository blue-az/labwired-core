# Fidelity scoreboard — walk-free / `rec_tick=512` families

**Date:** 2026-07-27  
**Scope:** Per-family certification status for walk deletion + batched
`peripheral_tick_interval` (target `RECOMMENDED_TICK_INTERVAL` = 512) under
`--features event-scheduler`.

Legend:

| Status | Meaning |
|--------|---------|
| **green** | walk≡sched certified where claimed; EasyDMA / time-sensitive paths proven at tick 512 (Machine gate, not forced walk) |
| **interim** | known lag, thin model, or partial surface; walk-free may still hold |
| **blocked** | not certified for walk-free / 512 (forcer remains or no gate) |

---

## nRF52840 (PR-B)

| Surface | Status | Notes / gates |
|---------|--------|---------------|
| Forcer emptiness / `max_safe=512` | **green** | `tick_interval_inventory::nrf52840_dk_is_walk_free_and_tick_512` |
| TIMER COMPARE via Machine@512 | **green** | `nrf52840_timer_machine_gate` |
| TIMER walk@1≡sched@512 | **green** | `nrf52_timer_walk_differential::timer0_compare_walk1_vs_sched512_cycle_identity` |
| RTC COMPARE (EVTEN+INTEN) walk@1≡sched@512 | **green** | `nrf52_timer_walk_differential::rtc0_compare_walk1_vs_sched512_cycle_identity` |
| RADIO TX→END walk@1≡sched@512 | **green** | `nrf52_timer_walk_differential::radio_tx_end_walk1_vs_sched512_cycle_identity` (SHORTS READY_START + short countdown) |
| UARTE EasyDMA TX @512 | **green** | delay-0 dual-path; `nrf52_easydma_tick512_fidelity` (≤8 cycles; walk@1≡sched@512 within 1) |
| SAADC EasyDMA SAMPLE @512 | **green** | delay-0 dual-path; same fidelity test |
| PWM SEQSTART EasyDMA @512 | **green** | delay-0 dual-path; same fidelity test |
| SPIM EasyDMA (nRF) @512 | **green** | delay-0 in `spi.rs` + serial_instance mux |
| TWIM / ECB | **green** | already dual-path / scheduler before this work |
| RTC COUNTER poll-only | **interim** | advances on write/`sync_to`; no poll-only differential gate (compare IRQ path certified above) |
| RADIO bit-rate timing | **interim** | TX→END cycle identity green; full MODE/length bit-rate matrix not claimed |
| Analog / unmodelled blocks | **interim** | FICR/UICR/stubs etc. — inert Class-A |

**Before (EasyDMA):** completion via `bus_tick_indices` only → lag up to one
512-cycle batch after STARTTX/SAMPLE/SEQSTART.  
**After:** delay-0 scheduler event → completion on the next cycle under Machine
+ walk-free + interval 512. `tick_with_bus` retained for bare-bus unit tests.

---

## RP2040 (PR-C)

| Surface | Status | Notes / gates |
|---------|--------|---------------|
| Forcer emptiness / `max_safe=512` | **green** | `rp2040_pico_is_walk_free_and_tick_512` |
| TIMER ALARM0 via Machine@512 | **green** | `rp2040_timer_machine_gate` |
| DMA / PIO / USBCTRL | **green** | Class-B scheduler chains (delay-1 where noted in inventory) |
| SPI Class-A (write-side PL022) | **green** | inert walk-free; loopback completes inside `SSPDR` writes (`needs_legacy_walk=false`) — not an EasyDMA timing certificate |
| I2C Class-A (write-side DW) | **green** | inert walk-free; address-NACK abort inside `IC_DATA_CMD` writes — not a multi-slave/timing certificate |
| UART baud / paced DMA detail | **interim** | scheduler model present on the pico bus; no tick-512 baud or paced-transfer differential gate (honest thin model) |

---

## STM32H563 (PR-D)

| Surface | Status | Notes / gates |
|---------|--------|---------------|
| Forcer emptiness / `max_safe=512` | **green** | `tick_interval_inventory::h563_is_walk_free_and_tick_512` (`flash_models_ops` does not pin tick interval) |
| Zephyr boot (SysTick + console) | **green** | `stm32h563_zephyr_boot_walk_vs_scheduler_is_byte_identical` |
| GPDMA mem2mem TC @ interval 1 | **green** | `gpdma_mem2mem_tcie_is_byte_identical_at_interval_1` — walk≡sched per-instruction (dst bytes, CSR TCF, TC ISR) |
| GPDMA mem2mem @ interval 512 | **green** | `gpdma_mem2mem_is_byte_identical_at_interval_512` — both lanes @512 batched final state identical (relative delay-1 paces N× in both lanes) |
| RTC second + Alarm A @ interval 1 | **green** | `rtc_second_and_alarm_is_byte_identical_at_interval_1` — TR advance + ALRAF/ISR byte-identical |
| RTC second/alarm count @ interval 512 | **green** | `rtc_second_count_is_exact_at_interval_512` — absolute second deadlines; final TR + ISR count exact vs walk@1 |
| `flash_models_ops` | **interim** | still forces CPU quantum 1 (not tick interval) |
| FDCAN single-node (no CanBus) | **green** | intentional: TX-defer + level IRQ via scheduler; `needs_legacy_walk=false` so demo bus stays walk-free (`h563_is_walk_free_and_tick_512`; unit: `fdcan::single_node_is_walk_free_under_event_scheduler`) |
| FDCAN multi-node (CanBus `bus_rx` attached) | **interim** | **blocked for walk-free / 512**: `needs_legacy_walk` returns true (mpsc RX polled on the walk, bxCAN contract). Multi-node buses pin `max_safe=1` until `bus_rx` is event-driven with dual-lane walk@1≡sched@512 proof. Do **not** hatch walk-free — that starves RX. Gates: `fdcan::canbus_attach_forces_legacy_walk_for_rx_poll`, `fdcan::canbus_path_tx_sends_and_rx_drains_on_tick` |
| SPI wire timing | **interim** | bit engine on scheduler; family-wide EasyDMA@512 not claimed |

---

## ESP32-S3 (PR-E)

| Surface | Status | Notes / gates |
|---------|--------|---------------|
| Forcer emptiness / `max_safe=512` | **green** | inventory on `configure_xtensa_esp32s3` + recompute |
| Class-B engines (timers, GDMA, …) | **interim** / **green** per engine | see inventory Class-B notes; no single EasyDMA@512 matrix |
| WiFi / radio | **blocked** / **interim** | not a walk-free fidelity certificate |

---

## Cross-cutting

- **Feature-off builds:** honest `max_safe=1` (no event-scheduler drain).
- **Do not claim wall-clock millis** from cycle budgets; gates assert cycle
  identity / completion latency in device cycles only.
- Inventory narrative: `docs/performance/2026-07-27-tick1-walk-inventory.md`.
