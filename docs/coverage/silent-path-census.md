# Silent-path census

Measured at commit `d503501d323fe25fb9dc2dd87481806e37beeb67` (`origin/main`), branch
`chore/silent-path-census`, host `aarch64-apple-darwin`, `rustc 1.97.1`.

This document is a **measurement**. Nothing is fixed, escalated, or gated here. It
exists to scope three follow-up repairs that could not be sized without data.

## What was counted, and how it is gated

All counters live behind the `silent-census` Cargo feature
(`crates/core/src/census.rs`). It is **off by default**, is not implied by any other
feature, and even when compiled in it writes nothing unless `LABWIRED_CENSUS_OUT`
names an output path at runtime. Both the compile-time feature and the env var are
required, so it cannot be turned on by accident.

```
cargo build -p labwired-cli --features silent-census
LABWIRED_CENSUS_OUT=census.json ./target/debug/labwired test --script <lab>.yaml ...
```

| # | Path | Instrumented at | Sites |
|---|------|-----------------|-------|
| a | Dropped Cortex-M memory errors | `cpu/cortex_m.rs` - every `let _ = bus.write_*` and every discarded `bus.read_*`, wrapped in `census_bus!` | 64 (25 write, 39 read) |
| b | Stub-peripheral fallthrough | `bus/from_config.rs` - the `_other =>` arm ending the factory chain | 1 |
| c | Undecoded register access | `peripherals/**` - catch-all `_ => {}` / `_ => 0` decode arms, via `census_reg!` | 312 |
| c'| Undecoded register access, declarative models | `peripherals/declarative.rs` - the `reg_index_at` miss fallthrough | 2 |

### Why c' exists, and what the audit's arm count really contains

The audit sized (c) as "~204 `_ => {}` and ~173 `_ => 0` arms". A grep at this SHA
finds 201 and 176 (377 total). Classifying each by the *subject of its enclosing
`match`* shows they are not all register decodes:

| Arm population | Count |
|---|---|
| Total `_ => {}` / `_ => 0,` arms | 377 |
| ... in `#[cfg(test)]` / `mod tests` code | 19 |
| ... matching on a register offset (`offset`, `reg`, `reg_off`, `word_off`, ...) - **instrumented** | **312** |
| ... matching on something else entirely (`cmd`, `dest`, `src`, `self.state`, `self.pointer`, `upper.as_str()`, ...) - **not instrumented, not a register decode** | 46 |

Instrumenting the last group would have produced numbers that look like register
gaps but are not. They are excluded deliberately.

Separately, the `_ =>` grep **structurally cannot see** the declarative/SVD-driven
models (`GenericPeripheral`), whose decode is
`if let Some(idx) = self.reg_index_at(offset) { ... }` with a bare `Ok(0)` / `Ok(())`
fallthrough - the same silent path in a different shape. 138 of the 1,186 peripheral
instances across the runnable corpus are declarative, so omitting them would have made
a near-zero (c) result misleading. They are counted and reported separately as
`shape: declarative_miss`.

### Read the raw counts with a 4x byte multiplier

`Peripheral::read`/`write` are **byte**-granular; `read_u32`/`write_u32` decompose into
four byte accesses. Several models (RCC among them) additionally do a read-modify-write
per byte. So **one** 32-bit write to an undecoded register costs 4 `write` hits *and* 4
`read` hits. This is pinned by a test
(`crates/core/tests/census_probe.rs::raw_counts_carry_a_four_times_byte_multiplier`).
Divide raw (c) counts by 4 to get register-level accesses.

## Coverage

| | Count |
|---|---|
| Test scripts discovered under `examples/` (any YAML with an `assertions:` block) | **97** |
| Ran | **68** |
| Skipped - firmware artifact absent | **27** |
| Ran but produced no census file | **2** |

Of the 68 runs: **19 clean** (all counters zero), **49 hot**.
Assertion outcome 58 pass / 10 fail; the failures are pre-existing at this
SHA and are not caused by the census - see *Behavioural neutrality*, which proves
byte-identical output on failing labs as well as passing ones.

### On the brief's denominator

The brief describes the corpus as "89 projects and 82 committed `.elf` files" in
`examples/`. The 89 directories are right; the 82 ELFs are **repo-wide**. Only **8**
committed ELFs are under `examples/` - 67 are under `tests/fixtures/`. The runnable
unit is a *test script*, not a project: the 89 directories yield 97 scripts, and most
firmware comes from cross-compiling workspace members (33 of 38 ARM build units built
cleanly here; the 5 failures are 3 nested workspaces that do not build on this
toolchain and 2 crate names that are `[[bin]]` targets inside other packages, both of
which were then built via their real package).

## Aggregate: counter (a) - dropped Cortex-M memory errors

**2 hits, 2 distinct (pc, addr, kind), in 1 of 68 runs.**

| count | pc | addr | kind |
|---|---|---|---|
| 1 | `0x00000000` | `0x00000001` | read |
| 1 | `0x00000000` | `0x00000004` | read |

Both hits are in `examples/ci/dummy-memory-violation.yaml` - a fixture whose *purpose*
is to provoke a memory violation. **No shipped lab drops a single Cortex-M memory error.**

Cross-checked against the independent, always-on `fidelity::unmapped_mmio` log, which
records the same bus rejections one layer lower (per byte, at the point the error is
*created* rather than dropped):

| lab | fidelity `unmapped_mmio` (bytes rejected) | census (a) (errors dropped) |
|---|---|---|
| `ci/dummy-memory-violation` | 7 | 2 |
| `nucleo-l073rz/io-smoke` | 1 | 0 |
| `pico2/io-smoke` | 1 | 0 |
| `pico2/uart-smoke` | 1 | 0 |
| **total** | **10** | **2** |

The gap is the point: 8 of the 10 rejections were **propagated**, not dropped - they
hit the instruction-fetch path (`bus.read_u16(fetch_pc)?`, one of the 9 places
`cortex_m.rs` does use `?`), which already faults correctly. Counter (a) is measuring
the drop sites specifically, and they are cold.

## Aggregate: counter (b) - stub-peripheral fallthrough

**163 instantiations, 10 distinct `type:` strings, in 47 of 68 runs.**
This is the hot counter, and the histogram decides the fix.

| count | `type:` string | reading |
|---|---|---|
| 120 | `stub` | **Intentional.** The manifest literally says `type: stub`; the factory is doing what it was asked. |
| 22 | `nvic` | **Not intentional.** A real ARM core block, silently stubbed at the bus while the CPU models it internally. |
| 4 | `nrf54l_dppic_stub` | Named `*_stub` in the manifest, so intentional by convention - but it reaches the fallthrough by *failing to match*, not by being declared. |
| 4 | `nrf54l_wdt_stub` | Named `*_stub` in the manifest, so intentional by convention - but it reaches the fallthrough by *failing to match*, not by being declared. |
| 3 | `scb` | **Not intentional.** Same as `nvic` - a real ARM core block reaching the fallthrough. |
| 2 | `nrf54l_ficr_stub` | Named `*_stub` in the manifest, so intentional by convention - but it reaches the fallthrough by *failing to match*, not by being declared. |
| 2 | `nrf54l_regulators_stub` | Named `*_stub` in the manifest, so intentional by convention - but it reaches the fallthrough by *failing to match*, not by being declared. |
| 2 | `nrf54l_rramc_stub` | Named `*_stub` in the manifest, so intentional by convention - but it reaches the fallthrough by *failing to match*, not by being declared. |
| 2 | `nrf54l_tampc_stub` | Named `*_stub` in the manifest, so intentional by convention - but it reaches the fallthrough by *failing to match*, not by being declared. |
| 2 | `nrf54l_uicr_stub` | Named `*_stub` in the manifest, so intentional by convention - but it reaches the fallthrough by *failing to match*, not by being declared. |

**Scoping consequence.** Turning the `_other =>` arm into a hard error today would
break 47 of 68 runnable labs. The 10 strings split three ways:

- **1 string, 120 hits (74%)** - literal `stub`. Declared intentional in the manifest.
- **7 strings, 18 hits** - the `nrf54l_*_stub` family. Intentional *by naming
  convention*, but they still reach the arm by failing every match, so the factory
  cannot tell them from a typo.
- **2 strings, 25 hits** - `nvic` and `scb`. Real ARM core blocks, stubbed at the bus
  in 22 and 3 labs respectively. These are the only genuinely surprising entries, and
  they are the ones worth a datasheet look.

So the fix is a migration, not a flag flip: make the factory *match* an explicit
`type: stub` (and the `*_stub` convention) so intentional stubbing is declared rather
than inferred from falling off the end, then hard-error the remainder. That is one PR
for the allowlist plus a look at `nvic`/`scb` - not fifteen.

## Aggregate: counter (c) - undecoded register access

**11 raw hits, 5 distinct (peripheral, offset, kind), in 5 of 68 runs.**

Applying the multiplier per entry rather than in bulk: the two 4-hit entries are one
32-bit write each; the three 1-hit entries are single **byte** writes. So the corpus
performs **5 register-level undecoded accesses in total**, all writes.

Fewer than 20 distinct pairs exist, so this is the complete list, not a top-20:

| count | peripheral | offset | kind | shape |
|---|---|---|---|---|
| 4 | `fdcan:Fdcan` | `0x0010` | write | `match_arm` |
| 4 | `iwdg:Iwdg` | `0x0008` | write | `match_arm` |
| 1 | `nrf54l.twim:Nrf54lTwim` | `0x0508` | write | `match_arm` |
| 1 | `nrf54l.twim:Nrf54lTwim` | `0x050c` | write | `match_arm` |
| 1 | `nrf52.twim:Nrf52Twim` | `0x0510` | write | `match_arm` |

Shape split: `match_arm` 11 hits, `declarative_miss` 0 hits.

**The declarative path recorded zero hits across the entire corpus.** That is a
measured zero from live instrumentation, not an untested assumption - the counter is
proven capable of firing by `census_probe.rs`.

**Scoping consequence.** Three findings are `twim` `0x508`/`0x50c`/`0x510` (nRF52 and
nRF54L), each a single byte write; one is `fdcan` `0x010` and one is `iwdg` `0x008`,
each a single 32-bit write. The WB55 precedent in `rcc.rs` is real, but
nothing in the current shipped corpus reproduces it: **no RCC/clock-enable offset is
undecoded on any lab that runs.** Escalating undecoded writes to a fault would break 5
labs, each for one register.

## Per-lab table

`a` = dropped Cortex-M memory errors, `b` = stub instantiations, `c` = undecoded
register hits (raw, pre-divide). Sorted hot first.

| script | status | steps | a | b | c | detail |
|---|---|---|---|---|---|---|
| `examples/stm32f411ceu6-blackpill/io-smoke.yaml` | pass | 5,000,000 | 0 | 13 | 4 | stub: `stub`x13; reg: `iwdg:Iwdg`@0x0008 writex4 |
| `examples/stm32f401cdu6-blackpill/i2c-smoke.yaml` | fail | 4,096 | 0 | 15 | 0 | stub: `stub`x15 |
| `examples/stm32f401cdu6-blackpill/io-smoke.yaml` | pass | 64 | 0 | 15 | 0 | stub: `stub`x15 |
| `examples/stm32f401cdu6-blackpill/trace-smoke.yaml` | pass | 64 | 0 | 15 | 0 | stub: `stub`x15 |
| `examples/stm32f401cdu6/uart-smoke.yaml` | pass | 64 | 0 | 15 | 0 | stub: `stub`x15 |
| `examples/nrf54l15-smart-ring/io-smoke.yaml` | pass | 500,000 | 0 | 9 | 2 | stub: `nrf54l_dppic_stub`x2, `nrf54l_wdt_stub`x2, `nrf54l_ficr_stub`x1, `nrf54l_regulators_stub`x1, `nrf54l_rramc_stub`x1, `nrf54l_tampc_stub`x1, `nrf54l_uicr_stub`x1; reg: `nrf54l.twim:Nrf54lTwim`@0x0508 writex1, `nrf54l.twim:Nrf54lTwim`@0x050c writex1 |
| `examples/nrf54l15-dk/io-smoke.yaml` | pass | 200,000 | 0 | 9 | 0 | stub: `nrf54l_dppic_stub`x2, `nrf54l_wdt_stub`x2, `nrf54l_ficr_stub`x1, `nrf54l_regulators_stub`x1, `nrf54l_rramc_stub`x1, `nrf54l_tampc_stub`x1, `nrf54l_uicr_stub`x1 |
| `examples/nucleo-l073rz/io-smoke.yaml` | fail | 0 | 0 | 5 | 0 | stub: `stub`x3, `nvic`x1, `scb`x1 |
| `examples/ci/l476-bldc-stall.yaml` | pass | 1,265,348 | 0 | 3 | 0 | stub: `nvic`x1, `scb`x1, `stub`x1 |
| `examples/h563-uds-ecu/uds-session-smoke.yaml` | pass | 2,000,000 | 0 | 1 | 2 | stub: `nvic`x1; reg: `fdcan:Fdcan`@0x0010 writex2 |
| `examples/h563-uds-ecu/uds-smoke.yaml` | pass | 2,000,000 | 0 | 1 | 2 | stub: `nvic`x1; reg: `fdcan:Fdcan`@0x0010 writex2 |
| `examples/nokia5110-invaders-lab/io-smoke.yaml` | pass | 5,000,000 | 0 | 3 | 0 | stub: `nvic`x1, `scb`x1, `stub`x1 |
| `examples/pico2/io-smoke.yaml` | fail | 0 | 0 | 3 | 0 | stub: `stub`x2, `nvic`x1 |
| `examples/pico2/uart-smoke.yaml` | fail | 0 | 0 | 3 | 0 | stub: `stub`x2, `nvic`x1 |
| `examples/ads1115-adc-lab/io-smoke.yaml` | pass | 500,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/ads1115-adc-lab/stimuli-smoke.yaml` | pass | 2,000,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/adxl345-sensor-lab/io-smoke.yaml` | pass | 200,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/bme280-weather-lab/io-smoke.yaml` | pass | 200,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/ci/dummy-memory-violation.yaml` | pass | 0 | 2 | 0 | 0 | arm: pc=0x00000000 addr=0x00000001 readx1, pc=0x00000000 addr=0x00000004 readx1 |
| `examples/demo-blinky/io-smoke.yaml` | pass | 10,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/ds3231-rtc-lab/io-smoke.yaml` | pass | 500,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/ds3231-rtc-lab/stimuli-smoke.yaml` | pass | 2,000,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/f103-fidelity-bench/gpiobug-smoke.yaml` | fail | 200,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/f103-i2c-silicon/io-smoke.yaml` | pass | 50,000,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/ili9341-tft-lab/io-smoke.yaml` | pass | 20,000,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/ina219-power-lab/io-smoke.yaml` | pass | 500,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/ina219-power-lab/stimuli-smoke.yaml` | pass | 2,000,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/max31855-thermocouple-lab/io-smoke.yaml` | fail | 200,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/mpu6050-sensor-lab/io-smoke.yaml` | pass | 200,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/neo6m-gps-lab/io-smoke.yaml` | pass | 200,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/ntc-thermistor-lab/io-smoke.yaml` | fail | 200,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/rp2040-pio/asm-smoke.yaml` | pass | 10 | 0 | 2 | 0 | stub: `nvic`x1, `stub`x1 |
| `examples/rp2040-pio/io-smoke.yaml` | fail | 10,000 | 0 | 2 | 0 | stub: `nvic`x1, `stub`x1 |
| `examples/ssd1306-hello-lab/io-smoke.yaml` | pass | 200,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/vl53l1x-tof-lab/io-smoke.yaml` | pass | 200,000 | 0 | 2 | 0 | stub: `stub`x2 |
| `examples/feather-f405/io-smoke.yaml` | pass | 64 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/feather-f405/uart-smoke.yaml` | pass | 64 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/h735-telematics-lab/io-smoke.yaml` | pass | 840,000 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/hil-displacement-showcase/io-smoke.yaml` | pass | 0 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/hil-displacement-showcase/showcase-test.yaml` | pass | 0 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/nucleo-f401re/io-smoke.yaml` | pass | 64 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/nucleo-f401re/uart-smoke.yaml` | pass | 64 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/nucleo-f767zi/io-smoke.yaml` | pass | 64 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/nucleo-f767zi/uart-smoke.yaml` | pass | 64 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/nucleo-h563zi/fullchip-smoke.yaml` | pass | 2,000 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/nucleo-h563zi/io-smoke.yaml` | pass | 5,000,000 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/nucleo-h563zi/uart-smoke.yaml` | pass | 64 | 0 | 1 | 0 | stub: `nvic`x1 |
| `examples/seeed-xiao-nrf52840-sense/uart-gpio-spi-smoke.yaml` | pass | 20,000 | 0 | 0 | 1 | reg: `nrf52.twim:Nrf52Twim`@0x0510 writex1 |
| `examples/stm32h735-smoke/io-smoke.yaml` | pass | 5,000,000 | 0 | 1 | 0 | stub: `nvic`x1 |

### Clean labs (all three counters zero)

| script | status | steps |
|---|---|---|
| `examples/ci-multiarch/two-riscv-test.yaml` | pass | 2,000 |
| `examples/ci/dummy-fail-uart.yaml` | fail | 10 |
| `examples/ci/dummy-max-cycles.yaml` | pass | 10 |
| `examples/ci/dummy-max-steps.yaml` | pass | 10 |
| `examples/ci/dummy-max-uart-bytes.yaml` | pass | 10,000 |
| `examples/ci/dummy-no-progress.yaml` | pass | 125 |
| `examples/ci/dummy-wall-time.yaml` | pass | 0 |
| `examples/ci/two-node-inputs-env.yaml` | pass | 10 |
| `examples/ci/uart-inject-echo.yaml` | pass | 20,000 |
| `examples/ci/uart-ok.yaml` | pass | 1,000 |
| `examples/esp32c3-blinky/test-blink.yaml` | pass | 800,000 |
| `examples/esp32c3-leo-airquality/test-fresh.yaml` | pass | 24,000,000 |
| `examples/esp32c3-leo-airquality/test-stuffy.yaml` | pass | 28,000,000 |
| `examples/esp32c3-leo-airquality/test.yaml` | pass | 28,000,000 |
| `examples/kw41z-cow-activity/calm.yaml` | pass | 6,000,000 |
| `examples/kw41z-cow-activity/stimulus-shake.yaml` | pass | 6,000,000 |
| `examples/nrf52840-proximity-lab/proximity-smoke.yaml` | fail | 6,000,000 |
| `examples/nrf52840-secure-boot-lab/secure-boot-smoke.yaml` | pass | 30,000,000 |
| `examples/simctl-selftest/simctl-selftest.yaml` | pass | 7,210 |

### Skipped - could not run

Every script that did not run gets a row. None is omitted.

| script | missing artifact | why |
|---|---|---|
| `examples/canmod-gps-sim/canmod-smoke.yaml` | `./firmware/build/canmod_gps_sim.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/ci/riscv-uart-ok.yaml` | `../../target/riscv32i-unknown-none-elf/release/riscv-ci-fixture` | riscv32 rustup target not installed on this host |
| `examples/esp32-bay-occupancy/tests/test-debounce.yaml` | `../firmware/.pio/build/adafruit_feather_esp32_v2/firmware.elf` | PlatformIO/Arduino output; needs `pio run`, not committed |
| `examples/esp32-bay-occupancy/tests/test-fault-and-display.yaml` | `../firmware/.pio/build/adafruit_feather_esp32_v2/firmware.elf` | PlatformIO/Arduino output; needs `pio run`, not committed |
| `examples/esp32-bay-occupancy/tests/test-nonblocking.yaml` | `../firmware/.pio/build/adafruit_feather_esp32_v2/firmware.elf` | PlatformIO/Arduino output; needs `pio run`, not committed |
| `examples/esp32-bay-occupancy/tests/test-occupancy-combinations.yaml` | `../firmware/.pio/build/adafruit_feather_esp32_v2/firmware.elf` | PlatformIO/Arduino output; needs `pio run`, not committed |
| `examples/esp32-bay-occupancy/tests/test-thresholds-hysteresis.yaml` | `../firmware/.pio/build/adafruit_feather_esp32_v2/firmware.elf` | PlatformIO/Arduino output; needs `pio run`, not committed |
| `examples/esp32c3-mlx90640-thermal/test-fault.yaml` | `./firmware/thermal_fingerprint.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/esp32c3-mlx90640-thermal/test-iolink-fault.yaml` | `./firmware/thermal_fingerprint.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/esp32c3-mlx90640-thermal/test-iolink.yaml` | `./firmware/thermal_fingerprint.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/esp32c3-mlx90640-thermal/test.yaml` | `./firmware/thermal_fingerprint.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/f103-fidelity-bench/clockbug-nogate-smoke.yaml` | `./firmware/build/clockbug.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/f103-fidelity-bench/clockbug-smoke.yaml` | `./firmware/build/clockbug.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/f103-fidelity-bench/control-smoke.yaml` | `./firmware/build/control.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/f103-fidelity-bench/rambug-smoke.yaml` | `./firmware/build/rambug.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/f103-j1939-monitor/j1939-replay.yaml` | `./firmware/build/j1939_monitor.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/f103-uds-ecu/firmware/diff/diff-smoke.yaml` | `/tmp/lw-deploy/core/examples/f103-uds-ecu/firmware/diff/build/f103_uds_diff.elf` | absolute path into an external HIL deploy tree |
| `examples/f103-uds-ecu/uds-reset-smoke.yaml` | `./firmware/build/f103_uds_ecu.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/f103-uds-ecu/uds-session-smoke.yaml` | `./firmware/build/f103_uds_ecu.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/f103-uds-ecu/uds-smoke.yaml` | `./firmware/build/f103_uds_ecu.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/h563-uds-bootloader/ota-smoke.yaml` | `../../../udslib/examples/h563_uds_bootloader/bootloader/build/h563_uds_bootloader_sim.elf` | sibling `udslib` checkout not present in this repo |
| `examples/iolink-dido/test.yaml` | `./firmware/iolink_dido.elf` | C/Makefile firmware (arm-none-eabi-gcc / riscv32-esp-elf-gcc); ELF not committed |
| `examples/marketplace-arduino-c3/stimuli-smoke.yaml` | `../../platformio/marketplace-arduino-c3/.pio/build/marketplace/firmware.elf` | PlatformIO/Arduino output; needs `pio run`, not committed |
| `examples/mb1355c/uart-smoke.yaml` | `./board_firmware/target/thumbv7em-none-eabi/release/firmware-mb1355c-demo` | nested cargo workspace; host build fails (`unwinding panics are not supported without std`) |
| `examples/nucleo-h563zi/golden-reference/dummy_test.yaml` | `target/thumbv7em-none-eabihf/release/firmware-h563-demo` | firmware artifact absent |
| `examples/nucleo_g474re/uart-smoke.yaml` | `./board_firmware/target/thumbv7em-none-eabi/release/firmware-nucleo_g474re-demo` | nested cargo workspace; host build fails (`unwinding panics are not supported without std`) |
| `examples/nucleo_wba52cg/uart-smoke.yaml` | `./board_firmware/target/thumbv8m.main-none-eabi/release/firmware-nucleo_wba52cg-demo` | nested cargo workspace; host build fails (`unwinding panics are not supported without std`) |

### Ran, but produced no census file

| script | why |
|---|---|
| `examples/ci/benchmark.yaml` | exited before the census dump point (pre-existing config error) |
| `examples/stm32f103-integrated-test/stm32f103_integrated_test.yaml` | exited before the census dump point (pre-existing config error) |

## Behavioural neutrality

The instrumentation must not change what the simulator does, or the census is
worthless. Two independent arguments:

**1. By construction.** With the feature off, every recording site expands back to the
code it wraps:

| macro | feature off expands to |
|---|---|
| `census_bus!(self, kind, expr)` | `expr` - the bare expression |
| `census_reg!(name, off, kind)` | `()` - so `_ => { census_reg!(..); }` is `_ => {}` and `_ => { census_reg!(..); 0 }` is `_ => 0` |
| `census::record_stub` / `record_undecoded_reg_named` / `dump_if_requested` | empty `#[inline(always)]` fns |

The macro arguments are not even evaluated when the feature is off, so a site cannot
introduce a side effect, a panic, or a borrow. With the feature on, every arm still
performs its original action and *then* records; no control flow changed anywhere.

**2. Empirically.** Eight labs run twice - once with a feature-off binary, once with a
feature-on binary that was *actively recording* - and `result.json` (status, steps,
cycles, assertions, full CPU register state, UART bytes, peripheral inspection)
compared byte-for-byte:

| lab | status | `result.json` | census hits recorded on the ON run (a, b, c) |
|---|---|---|---|
| `examples/adxl345-sensor-lab/io-smoke.yaml` | pass | **byte-identical** | 0, 2, 0 |
| `examples/h563-uds-ecu/uds-smoke.yaml` | pass | **byte-identical** | 0, 1, 2 |
| `examples/ci/dummy-memory-violation.yaml` | pass | **byte-identical** | 2, 0, 0 |
| `examples/nucleo-h563zi/fullchip-smoke.yaml` | pass | **byte-identical** | 0, 1, 0 |
| `examples/pico2/io-smoke.yaml` | **fail** | **byte-identical** | 0, 3, 0 |
| `examples/nucleo-l073rz/io-smoke.yaml` | **fail** | **byte-identical** | 0, 5, 0 |
| `examples/nrf54l15-smart-ring/io-smoke.yaml` | pass | **byte-identical** | 0, 9, 2 |
| `examples/kw41z-cow-activity/stimulus-shake.yaml` | pass | **byte-identical** | 0, 0, 0 |

The proof is deliberately non-vacuous: all three counters fired somewhere in this set
(counter a on `dummy-memory-violation`, b on six labs, c on two), and both pass and
fail outcomes are represented, yet output never moved a byte. Exit codes matched too.

## What was NOT measured

- **46 catch-all arms that match on something other than a register offset** - `cmd`,
  `dest`, `self.state`, `upper.as_str()` and similar. Not register decodes; counting
  them would fabricate gaps.
- **19 catch-all arms inside `#[cfg(test)]` / `mod tests`.**
- **27 scripts whose firmware could not be produced on this host** - PlatformIO/Arduino
  builds, C/Makefile firmware needing `arm-none-eabi-gcc` or `riscv32-esp-elf-gcc`, one
  riscv32 target not installed, three nested cargo workspaces that fail to build on
  this toolchain, and two paths pointing outside the repo. Every one is a row above.
- **Xtensa and ESP32 ROM-boot paths.** No `examples/esp32s3-*` directory has a test
  script at all, and the C3 Arduino lab needs `LABWIRED_ESP32C3_*` flash/ROM images.
- **The RISC-V CPU's own error handling.** `cpu/riscv.rs` propagates ~32 bus results
  with `?` and discards none, so it has no (a)-equivalent to count. Unverified beyond
  reading the code.
- **`Machine::run` batched orchestration.** Every run here used the CLI's default
  per-instruction path. The browser's batched path was not measured.
- **Whether an undecoded offset is actually *wrong*.** The census says the model did
  not decode an offset the firmware touched. Deciding whether that matters needs the
  datasheet, one register at a time. That is the follow-up work this table scopes.

## Belongs to another task

- Fixing any of the three paths. Explicitly out of scope here.
- `examples/ci/benchmark.yaml` declares `max_steps: 1000000000`, above the CLI's
  `MAX_ALLOWED_STEPS` of 50000000, so it cannot run as committed.
- `examples/stm32f103-integrated-test` points at a system manifest missing a `chip:`
  field and fails to load.
- `examples/f103-uds-ecu/firmware/diff/diff-smoke.yaml` hardcodes absolute
  `/tmp/lw-deploy/...` paths and can never run from a clean checkout.
- 10 of the 68 runnable labs fail their own assertions at this SHA. Pre-existing,
  unrelated to this work, and not investigated.
- `scripts/example_smokes.sh` globs `examples/*/*smoke*.yaml examples/*/test*.yaml`,
  which misses ~7 scripts one level deeper plus every `examples/ci/*.yaml`. Widening
  it is a separate change.

