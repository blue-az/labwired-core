# Plan 4 — ESP32-S3 I²C + TMP102 sensor demo

**Status:** Design approved 2026-04-30. Awaiting written-spec review before implementation plan.

**Goal:** Run an unmodified `esp-hal` firmware in the LabWired simulator that periodically reads a simulated TMP102 temperature sensor over I²C, prints the reading over USB-Serial-JTAG, and toggles GPIO2 when temperature crosses 30 °C. The same firmware must run on a real ESP32-S3-Zero, with HW-oracle parity validation at the I²C controller register level.

**Builds on:** Plan 1 (XtensaLx7 CPU), Plan 2 (boot/UART/SYSTIMER), Plan 3 (GPIO + intmatrix + blinky), Plan 1.5 (hw-oracle harness, PR #82).

## Scope

In scope:

- ESP32-S3 I²C0 controller peripheral (command-list executor, sufficient for `esp-hal::i2c::master::I2c`).
- TMP102 device modeled as an `I2cDevice` with drifting temperature.
- esp-hal firmware combining I²C + GPIO + SYSTIMER + JTAG.
- Sim end-to-end test mirroring `e2e_blinky.rs`.
- HW oracle controller-register-level tests (no physical TMP102 required).
- Manual runbook for end-to-end test on real hardware with a physical TMP102.

Explicitly out of scope:

- ESP32-S3 I2C1 (only I2C0).
- I²C slave mode.
- Multi-master arbitration, SCL clock-stretching fidelity, NACK retry policies.
- SPI controller (separate plan).
- HW oracle tests for byte-level I²C bus traffic (would need a wired TMP102 plus a logic-analyzer integration the project doesn't have).

## User-facing demo

Acceptance criteria for the demo, observable through `labwired run`:

1. JTAG stdout contains at least four lines of the form `T = NN.NN °C` over a 5 s simulated run.
2. The first line prints approximately 25.00 °C; subsequent lines drift upward by 0.5 °C per tick.
3. GPIO2 transitions from 0 to 1 once the printed temperature crosses 30 °C, then back to 0 once it wraps below 30 °C.
4. The same firmware ELF, flashed to a real ESP32-S3-Zero with a TMP102 wired to GPIO8 (SDA) / GPIO9 (SCL), prints real temperatures and toggles GPIO2 on threshold (manual L4 test only).

## Architecture

Six pieces, each independently testable:

1. **`crates/core/src/peripherals/esp32s3/i2c.rs`** (~400-500 LoC). ESP32-S3 I²C0 controller modeled as a `Peripheral`. Memory-mapped at base `0x6001_3000` with size 4 KiB. Implements the command-list engine described in the I²C engine section below. Owns its attached slave devices: `Vec<Box<dyn I2cDevice>>`, addressed by 7-bit slave address.

2. **`crates/core/src/peripherals/esp32s3/tmp102.rs`** (~80 LoC). New TMP102 implementation as an `I2cDevice` (the existing `crates/core/src/peripherals/i2c_temp_sensor.rs` is left untouched — it remains a memory-mapped variant for STM32 demos). Tracks the pointer register set by writes. On reads at the temperature pointer, returns drifting temperature MSB-first; on reads at config / T_LOW / T_HIGH, returns canned values.

3. **`crates/core/src/system/xtensa.rs` extension** (~30 LoC). `configure_xtensa_esp32s3` registers the I²C0 peripheral on the bus with a TMP102 attached at address `0x48`. Wires the I²C0 IRQ source through the interrupt matrix, mirroring how SYSTIMER is wired in Plan 3.

4. **`examples/esp32s3-i2c-tmp102/`** (new esp-hal Cargo crate, ~100 LoC firmware). Excluded from the workspace exactly like `examples/esp32s3-blinky` (uses the `+esp` toolchain). SYSTIMER 1 Hz alarm, on each tick calls `I2c::write_read(0x48, &[0x00], &mut buf)`, decodes a 12-bit temperature from `buf`, prints it via `esp_println::println!`, and sets GPIO2 high if T > 30 °C, low otherwise.

5. **`crates/core/tests/e2e_i2c_tmp102.rs`** (~80 LoC, gated on the `esp32s3-fixtures` feature). End-to-end test: builds the example firmware, runs it through the simulator for ~5 s of simulated time, asserts the JTAG stdout contains the expected temperature lines and GPIO2 transitioned at the expected cycle.

6. **`configs/chips/esp32s3-zero.yaml`** (4 lines). Documents the new I²C0 peripheral entry. Like the existing entries this is documentation only; `configure_xtensa_esp32s3` is the authoritative wiring.

## I²C command-list engine

The ESP32-S3 I²C peripheral is a command-list machine: the firmware programs a list of commands (RSTART / WRITE / READ / STOP / END) into 16 command registers, pushes outgoing data into a TX FIFO, sets `CTR.TRANS_START`, and waits for `INT_RAW.TRANS_COMPLETE`.

### Register subset modeled

Only registers `esp-hal::i2c::master::I2c` actually touches:

| Offset | Register | Purpose |
|--------|----------|---------|
| `0x04` | `CTR` | `TRANS_START` lives at bit 5 |
| `0x10` | `SLAVE_ADDR` | 7-bit slave address (only `[6:0]` used) |
| `0x18` | `FIFO_DATA` | Write to push TX, read to pop RX |
| `0x1C` | `FIFO_CONF` | Reset bits accept and clear; no behavior beyond bookkeeping |
| `0x20` | `INT_RAW` | Raw interrupt status |
| `0x24` | `INT_CLR` | Write 1 to clear corresponding INT_RAW bits |
| `0x28` | `INT_ENA` | Interrupt enable mask |
| `0x2C` | `INT_ST` | `INT_RAW & INT_ENA` |
| `0x44` | `FIFO_ST` | TX/RX FIFO levels |
| `0x58..0x94` | `CMD0..CMD15` | 14-bit command words (16 entries × 4 B) |

Other registers (`SCL_LOW_PERIOD`, `SCL_HIGH_PERIOD`, `TIMEOUT`, etc.) accept writes and ignore them.

### Command word format (per ESP32-S3 TRM §29)

| Bits | Field | Notes |
|------|-------|-------|
| `[13:11]` | `opcode` | `0=RSTART, 1=WRITE, 2=READ, 3=STOP, 4=END` |
| `[10]` | `ack_check_en` | Accepted, not enforced (no NACK from sim slave) |
| `[9]` | `ack_exp` | Write only |
| `[8]` | `ack_val` | Read only |
| `[7:0]` | `byte_num` | Byte count for WRITE / READ |

### Execution semantics on `CTR.TRANS_START` write

When firmware writes `CTR` with bit 5 set, the simulator immediately walks the command list (instantaneous from the firmware's perspective):

1. Walk `CMD0` to `CMD15`, stopping at the first `END` or `STOP`.
2. For `WRITE byte_num=N`: pop N bytes from TX FIFO. The first byte after an `RSTART` is interpreted as `(addr<<1) | (W=0/R=1)` and selects the active slave by address bits `[7:1]`. Subsequent bytes are delivered to the active slave via `I2cDevice::write(byte)`.
3. For `READ byte_num=N`: call `I2cDevice::read()` on the active slave N times; push each returned byte to RX FIFO.
4. For `RSTART`: call `I2cDevice::start()` on the active slave (no-op if no slave yet selected) and clear the active slave selection so the next byte addresses a (possibly new) slave.
5. For `STOP`: call `I2cDevice::stop()` on the active slave.
6. On completion (END or STOP reached): set `INT_RAW.TRANS_COMPLETE` (bit 7). If `INT_ENA.TRANS_COMPLETE` is set, the I²C0 IRQ source fires through the interrupt matrix.

Reading `FIFO_DATA` pops one byte from RX FIFO. Writing `FIFO_DATA` pushes one byte to TX FIFO. `FIFO_ST` reflects current levels.

### Intentional simplifications

- **No SCL/SDA timing.** Bus cycles are zero — the entire transaction completes synchronously when `TRANS_START` is written.
- **No NACK / arbitration loss.** The simulated slave always ACKs.
- **No bus busy detection.** `BUSY` always reads 0.
- **No slave mode.** Controller only.
- **No clock divider effects.** Bytes always travel.

These match the existing UART model (Plan 2): the simulator gives the firmware a "well-behaved" peripheral and lets HW oracle catch any divergence that matters.

## TMP102 device

`crates/core/src/peripherals/esp32s3/tmp102.rs` implements the existing `I2cDevice` trait from `crates/core/src/peripherals/i2c.rs`:

```rust
pub trait I2cDevice: Send {
    fn address(&self) -> u8;
    fn read(&mut self) -> u8;
    fn write(&mut self, data: u8);
    fn start(&mut self) {}
    fn stop(&mut self) {}
}
```

State:

- `pointer: u8` — current pointer register, set by `write`. Default 0.
- `temp_raw: i16` — drifting 12-bit temperature value, left-justified to fill 16 bits. Initialized to 0x1900 (25.0 °C).
- `read_phase: u8` — 0 means next read returns MSB, 1 means next read returns LSB. Reset to 0 on every `start()`.

Behavior:

- `write(b)` with no prior writes since last `start()`: set `pointer = b`. Subsequent writes overwrite higher bytes of config / T_LOW / T_HIGH per pointer (ignored for the demo since firmware never writes those, but spec is explicit).
- `read()` at pointer 0x00 (temperature): on `read_phase=0`, return `(temp_raw >> 8) as u8` and advance to phase 1. On phase 1, return `(temp_raw & 0xFF) as u8`, advance phase back to 0, and tick `temp_raw += 0x80` (i.e. +0.5 °C, since 1 LSB = 0.0625 °C and the value is left-shifted by 4). When `temp_raw` exceeds 0x2300 (35 °C), wrap back to 0x1400 (20 °C).
- `read()` at pointer 0x01 (config): canned 0x60A0 high byte then low byte.
- `read()` at pointer 0x02 (T_LOW): 0x4B00 (75 °C) high byte then low byte.
- `read()` at pointer 0x03 (T_HIGH): 0x5000 (80 °C) high byte then low byte.
- `start()` resets `read_phase` to 0.
- `stop()` is a no-op.

The drift uses 0.5 °C steps so the threshold-crossing demo is visible within ~10 ticks.

## Firmware

`examples/esp32s3-i2c-tmp102/src/main.rs`:

```rust
#![no_std]
#![no_main]

use core::cell::RefCell;
use critical_section::Mutex;
use esp_backtrace as _;
use esp_hal::{
    gpio::{Level, Output, OutputConfig},
    handler, main,
    i2c::{master::I2c, master::Config as I2cConfig},
    time::Duration,
    timer::{systimer::SystemTimer, PeriodicTimer},
};
use esp_println::println;

const TMP102_ADDR: u8 = 0x48;
const THRESHOLD_C: f32 = 30.0;

static TICK_FLAG: Mutex<RefCell<bool>> = Mutex::new(RefCell::new(false));
static ALARM: Mutex<RefCell<Option<PeriodicTimer<'static, esp_hal::Blocking>>>> =
    Mutex::new(RefCell::new(None));

#[handler]
fn alarm_isr() {
    critical_section::with(|cs| {
        TICK_FLAG.replace(cs, true);
        if let Some(alarm) = ALARM.borrow_ref_mut(cs).as_mut() {
            alarm.clear_interrupt();
        }
    });
}

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());

    let mut led = Output::new(p.GPIO2, Level::Low, OutputConfig::default());

    let mut i2c = I2c::new(p.I2C0, I2cConfig::default())
        .unwrap()
        .with_sda(p.GPIO8)
        .with_scl(p.GPIO9);

    let st = SystemTimer::new(p.SYSTIMER);
    let mut alarm = PeriodicTimer::new(st.alarm0);
    alarm.set_interrupt_handler(alarm_isr);
    alarm.start(Duration::from_millis(1000)).unwrap();
    alarm.listen();

    critical_section::with(|cs| {
        ALARM.replace(cs, Some(alarm));
    });

    loop {
        let tick = critical_section::with(|cs| {
            let v = *TICK_FLAG.borrow_ref(cs);
            TICK_FLAG.replace(cs, false);
            v
        });
        if tick {
            let mut buf = [0u8; 2];
            if i2c.write_read(TMP102_ADDR, &[0x00], &mut buf).is_ok() {
                let raw = ((buf[0] as i16) << 8) | (buf[1] as i16);
                let temp_c = (raw >> 4) as f32 * 0.0625;
                println!("T = {:.2} °C", temp_c);
                if temp_c > THRESHOLD_C {
                    led.set_high();
                } else {
                    led.set_low();
                }
            }
        }
        core::hint::spin_loop();
    }
}
```

Pins GPIO8 / GPIO9 are the ESP32-S3-Zero default I²C0 pins. The simulator's IO mux model treats `with_sda`/`with_scl` writes as accept-and-record (no internal multiplexing matters because the simulated TMP102 is wired directly to the I²C0 controller, not to GPIO pins). The same firmware therefore works unmodified on real silicon (where the IO mux genuinely matters) and in sim.

## Data flow

End-to-end path of one temperature read at simulated cycle ≈ 80 M (1 s):

1. SYSTIMER alarm fires; intmatrix routes SYSTIMER_TARGET0 → CPU IRQ.
2. ISR sets `TICK_FLAG = true` and clears the alarm interrupt.
3. Main loop drains `TICK_FLAG`, calls `I2c::write_read(0x48, &[0x00], &mut buf)`.
4. esp-hal pushes:
   - `CMD0 = RSTART`
   - `CMD1 = WRITE byte_num=2`  (slave addr+W, then pointer 0x00)
   - `CMD2 = RSTART`
   - `CMD3 = WRITE byte_num=1`  (slave addr+R)
   - `CMD4 = READ  byte_num=2`
   - `CMD5 = STOP`
5. TX FIFO ← `[0x90, 0x00, 0x91]`  (`0x48<<1`, pointer, `0x48<<1 | 1`).
6. Firmware writes `CTR.TRANS_START = 1`.
7. Simulator walks the command list:
   - `RSTART` → no slave yet selected.
   - `WRITE 2` → first byte `0x90` → select TMP102 (address bits = 0x48); second byte `0x00` → `tmp102.write(0x00)` → `pointer = 0`.
   - `RSTART` → `tmp102.start()`; clear active-slave selection.
   - `WRITE 1` → `0x91` → re-select TMP102, mode = READ.
   - `READ 2` → `tmp102.read()` × 2 → `[MSB, LSB]` pushed to RX FIFO.
   - `STOP` → `tmp102.stop()`.
   - Set `INT_RAW.TRANS_COMPLETE`.
8. Firmware polls `INT_ST`, drains RX FIFO into `buf`.
9. `temp = ((buf[0] as i16) << 8 | buf[1] as i16) >> 4` × 0.0625 °C.
10. `esp_println::println!("T = {:.2} °C", temp_c)` → JTAG TX FIFO → host stdout.
11. Threshold check toggles GPIO2; GpioObserver emits a transition event.

After about 11 reads, the firmware sees 30.5 °C and GPIO2 first goes high. After about 21 reads, the model's `temp_raw` wraps from 35.5 °C back to 20 °C and GPIO2 returns low. The cycle repeats roughly every 30 reads.

## Testing strategy

Four layers, each independent.

### L1. Sim unit tests (always run)

In-tree `#[cfg(test)]` tests, sub-second:

- `peripherals/esp32s3/i2c.rs`: command-list parsing, FIFO push/pop, INT_RAW transitions on TRANS_START, slave-not-found behavior (no panic, INT_RAW still completes), mid-list END termination. Approximately 10-15 tests.
- `peripherals/esp32s3/tmp102.rs`: pointer set/retain across reads, drift wraparound at 35 °C → 20 °C, MSB/LSB read order, `start()` resets read phase. Approximately 5 tests.

### L2. Sim e2e test (always run)

`crates/core/tests/e2e_i2c_tmp102.rs`, gated on the `esp32s3-fixtures` feature, mirrors `e2e_blinky.rs`. Boots the example firmware ELF in the simulator, runs ~5 s of simulated time, asserts:

- JTAG stdout contains at least 4 `T = NN.NN °C` lines.
- The temperatures are monotonically increasing across reads (modulo the 35 °C → 20 °C wrap).
- GPIO2 transitioned 0 → 1 at a cycle consistent with crossing 30 °C.

### L3. HW oracle controller-register-level test (gated `--features hw-oracle`)

Runs on the self-hosted ESP32-S3-Zero runner introduced in PR #82. Does **not** require a physical TMP102 — validates that the simulator's I²C0 controller register file behaves identically to real silicon at the register-write/read level.

Approximately 3-5 oracle tests using the existing `#[hw_oracle_test]` harness:

1. **Reset values:** read I²C0 register file via OpenOCD, compare against `i2c.rs` reset state. No CPU execution needed.
2. **Empty command list TRANS_COMPLETE:** load a tiny Xtensa program that writes `CTR.TRANS_START` with `CMD0 = END` and a single byte in TX FIFO, polls `INT_RAW`, BREAKs. Diff: same `INT_RAW` on sim and HW.
3. **NACK on missing slave:** program writes `RSTART; WRITE addr=0x48+W; END` with no slave on real bus, sets `TRANS_START`, polls `INT_RAW`. On HW: `INT_RAW.NACK_INT` is set; on sim with no TMP102 attached the same bit must set. (Sim-side: implement NACK behavior when no slave matches.)
4. **FIFO levels:** write 8 bytes to TX FIFO, read `FIFO_ST`, BREAK. Same level on sim and HW.

These tests catch any divergence between the simulator's command-list semantics and silicon at the register-write level.

### L4. HW end-to-end with physical TMP102 (manual, opportunistic)

`examples/esp32s3-i2c-tmp102/RUNBOOK.md` documents the manual procedure:

1. Wire a TMP102 (or pin-compatible: SHT30, LM75, etc. — note the protocol differences) to GPIO8 (SDA), GPIO9 (SCL), 3.3 V, GND.
2. `cargo run --release` from the example crate, which invokes `espflash` to flash and monitor.
3. Acceptance: real temperatures print on JTAG stdout, GPIO2 toggles on threshold (probe with logic analyzer or LED).

L4 is **not** in CI. It exists as documented evidence of end-to-end parity for any reviewer who happens to have the part.

## Risks and mitigations

| Risk | Mitigation |
|------|------------|
| Command-list semantics drift between sim and real silicon | L3 hw-oracle tests run on every push to main when self-hosted runner is online |
| esp-hal version updates change which I²C registers are touched | Pin esp-hal in `examples/esp32s3-i2c-tmp102/Cargo.toml`, mirroring how `examples/esp32s3-blinky` already pins it |
| TMP102 pointer reset semantics — real silicon retains pointer across STOPs | Model retains across both STOP and power-cycle (single source of truth in `tmp102.rs`); doctest documents the choice |
| First-peripheral hw-oracle pattern is novel | Keep L3 minimal (3-5 register-level tests); richer peripheral oracles come in follow-up plans |
| NACK behavior on real HW with no slave attached vs. ACK in sim with TMP102 attached | Sim implements both: TMP102 attached → ACK, no slave matches → NACK_INT bit set in `INT_RAW`; L3 oracle test 3 validates this against silicon |

## Out-of-scope (deferred)

- I²C interrupt-driven driver mode (firmware polls `INT_ST` for the demo; 1 Hz ticks make this fine).
- ESP32-S3 I2C1 controller (only I2C0 here).
- I²C slave mode.
- SCL clock-period fidelity, multi-master arbitration, clock stretching.
- SPI controller — separate plan.
- Peripheral-level hw-oracle tests for TMP102 itself (the project does not validate third-party device behavior).
- Logic-analyzer-driven byte-level I²C bus capture on real hardware.

## File inventory

New files:

```
crates/core/src/peripherals/esp32s3/i2c.rs         (~450 LoC)
crates/core/src/peripherals/esp32s3/tmp102.rs      (~80  LoC)
crates/core/tests/e2e_i2c_tmp102.rs                (~80  LoC)
examples/esp32s3-i2c-tmp102/Cargo.toml             (~30  LoC)
examples/esp32s3-i2c-tmp102/src/main.rs            (~100 LoC)
examples/esp32s3-i2c-tmp102/build.rs               (~10  LoC)
examples/esp32s3-i2c-tmp102/.cargo/config.toml     (~10  LoC)
examples/esp32s3-i2c-tmp102/rust-toolchain.toml    (~3   LoC)
examples/esp32s3-i2c-tmp102/README.md              (~50  LoC)
examples/esp32s3-i2c-tmp102/RUNBOOK.md             (~30  LoC)
crates/hw-oracle/tests/i2c_oracles.rs              (~120 LoC)
```

Modified files:

```
crates/core/src/peripherals/esp32s3/mod.rs         (+5  LoC)
crates/core/src/system/xtensa.rs                   (+30 LoC)
configs/chips/esp32s3-zero.yaml                    (+4  LoC)
Cargo.toml                                         (+1  LoC, exclude esp32s3-i2c-tmp102)
```

Total new content: ~1500-1700 LoC.

## Acceptance criteria

The plan is complete when:

1. `cargo test -p labwired-core` passes including new L1 unit tests.
2. `cargo test -p labwired-core --features esp32s3-fixtures e2e_i2c_tmp102` passes — the e2e test boots the firmware and observes the expected JTAG output and GPIO transitions.
3. `cargo test -p labwired-hw-oracle` passes (sim-side tests of the new I²C oracle bank).
4. The hw-oracle CI workflow (`.github/workflows/hw-oracle.yml`) runs on the self-hosted ESP32-S3-Zero and the L3 oracle tests pass against real silicon.
5. `examples/esp32s3-i2c-tmp102/RUNBOOK.md` documents the L4 manual procedure.
6. No regressions: existing `e2e_blinky`, `e2e_hello_world`, and the rest of the workspace test suite still pass.
