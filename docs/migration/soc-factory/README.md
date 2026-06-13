# SoC factory migration

Goal: every chip builds through the data-driven `SystemBus::from_config` path
(walk the chip YAML → per-family factory → place at base), with arch code
shrinking to a thin CPU-core overlay. Models stay statically linked; **topology
becomes data**. Today the Cortex-M and RISC-V chips already build this way; the
ESP32-S3 is the lone holdout, assembled by the 2186-line hand-wired
`system::xtensa::configure_xtensa_esp32s3`.

Strangler migration, each stage gated by the ESP32/Xtensa test suite as a golden
oracle (no behavior change until Stage 3):

- **Stage 0 — pin the oracle.** Baseline pass/fail of the ESP32/Xtensa test
  binaries (321 tests). See `STAGE0_golden_baseline.txt`.
- **Stage 1 — pre-stock the factory.** `peripherals/esp32s3/factory.rs::try_build`
  hosts the 26 ESP32-S3 peripheral types; `from_config` delegates to it before
  the generic match. Pure addition — no chip YAML references these types yet, so
  behavior is unchanged (regression identical to Stage 0). **← current**
- **Stage 2 — extract the Xtensa core overlay.** Carve the genuinely
  shared-CPU-state wiring (interrupt matrix, interconnect, boot ROM, RAM banks)
  out of `configure_xtensa_esp32s3` into `configure_xtensa_core`, ~150 lines.
- **Stage 3 — flip esp32s3 to `from_config`.** Complete `esp32s3.yaml` (all ~35
  peripherals), build via `from_config` + `configure_xtensa_core`, diff against
  the golden, then delete the per-peripheral body of `configure_xtensa_esp32s3`.
- **Stage 4 — repeat for esp32 / esp32c3.**
- **Stage 5 — feature-gate families** (`cortex-m`/`xtensa`/`esp32`/`nrf52`) so
  the wasm build and CI fixtures compile only what they need.

## Pattern

Per-family factories live in their own module (`peripherals/<family>/factory.rs`),
not inline in `bus::from_config`. This keeps the central match from growing and
lets it shrink as other families (nRF52, STM32) move out the same way.
