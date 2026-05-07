# Plan 5 — Dual-Core ESP32-S3 Design

**Date:** 2026-05-07
**Status:** approved design, awaiting plan
**Builds on:** Plan 1 (Xtensa LX7), Plan 2 (boot/UART/SYSTIMER), Plan 3 (GPIO/intmatrix), Plan 4 (I2C + TMP102)

## Goal

Bring up the ESP32-S3's second Xtensa LX7 core (APP_CPU) under faithful esp-hal bringup semantics, then ship a producer/consumer demo where PRO_CPU reads TMP102 + writes a shared `AtomicI32` and APP_CPU reads it + drives a GPIO LED on a temperature crossing.

## What we claim (and don't)

The simulator's existing claim — verified by the hw-oracle harness and stated in `README.md` — is **hardware-validated behavioral parity**: register state and observable peripheral behavior match real silicon.

Plan 5 extends this claim to dual-core: bringup register sequence, per-core PRID, atomic memory ordering primitives, and shared-memory observable behavior all match real silicon.

**Plan 5 does NOT claim:**
- Cycle accuracy of either core in isolation, nor of inter-core timing
- Cache modeling, region-aware load latency
- Pipeline hazards / branch prediction
- Inter-core software interrupts (`FROM_CPU_INTR_n`)
- Embassy-on-dual-core
- Full `RUNSTALL` mid-execution semantics (we model "started/stopped", not "stalled while running")

## Scope already covered by existing code

Two subsystems land in this plan with little or no work because they're already in tree:

1. **`MultiCoreMachine`** at `crates/core/src/multi_core.rs` already provides `Vec<Box<dyn Cpu>>`, `step_all()`, and IRQ routing (defaulting to core 0). It currently does 1-instruction-per-core round-robin and isn't wired into the ESP32-S3 system glue. Plan 5 extends it (quantum size, controller gating) and wires it in.
2. **Atomic instructions** (`S32C1I`, `L32AI`, `S32RI`) are decoded, executed, and HW-oracle-validated already (`crates/core/src/cpu/xtensa_lx7.rs:1095-1129`, `crates/core/tests/xtensa_exec.rs:3748-3831`). The demo's `AtomicI32::store(_, Release)` / `AtomicI32::load(_, Acquire)` will lower to these and Just Work.

## Architecture

### Outer loop shape

```
loop {
    cpu0.step_quantum(&mut bus, N=256);           // PRO_CPU
    if core_controller.is_app_cpu_released() {
        cpu1.step_quantum(&mut bus, N=256);       // APP_CPU
    }
    bus.tick_peripherals_n(N=256);                // advance peripherals
}
```

### Per-core state (each `XtensaLx7` instance)

- Independent register file, PS, EPCn, EXCSAVEn, CCOUNT
- Independent `PRID` SR — PRO_CPU = 0xCDCD, APP_CPU = 0xABAB
- Independent INTENABLE / INTERRUPT (per-core IRQ masks; only PRO takes IRQs in this demo)

### Shared state (one copy)

- `SystemBus`, all peripherals, all memory regions (IRAM/DRAM/flash-XIP)
- `intmatrix` already routes per-CPU (Plan 3); this demo only routes IRQs to PRO_CPU
- `CoreController` (new) — `{appcpu_entry: Option<u32>, runstall: bool, reset_en: bool, clkgate_en: bool}`. Released iff `entry.is_some() && !runstall && !reset_en && clkgate_en`. Shared via `Arc<Mutex<_>>` across rom_thunks, system_stub, and the outer loop.

### Single-core back-compat

If firmware never sets `appcpu_entry`, `cpu1` never steps; existing Plan 1–4 tests are byte-identical in observable behavior.

### Bringup sequence (real esp-hal `CpuControl::start_app_core`)

Three observable events the simulator must intercept:

1. **`ets_set_appcpu_boot_addr(addr)` ROM call.** Existing thunk at `crates/core/src/peripherals/esp32s3/rom_thunks.rs:195` is currently a NOP returning 0. Replace its body to read `a2` (first ABI arg = entry addr) and store it into the shared `CoreController.appcpu_entry`.
2. **Volatile store `SYSTEM.CORE_1_CONTROL_0[CLKGATE_EN] = 1`** at `0x600C_018C`, bit 0.
3. **Volatile store `SYSTEM.CORE_1_CONTROL_0[RESETTING] = 0`** at the same register, bit 2 (after a brief pulse to 1).

`SYSTEM_CORE_1_CONTROL_0_REG` (offset 0x18C from SYSTEM base 0x600C_0000) holds three relevant bits:
- bit 0 = `CONTROL_CORE_1_CLKGATE_EN`
- bit 1 = `CONTROL_CORE_1_RUNSTALL`
- bit 2 = `CONTROL_CORE_1_RESETTING`

`system_stub` will recognize writes to that one offset and route the bits into `CoreController` while still falling through to its backing store (so reads round-trip the written value).

On the released-edge transition, the outer loop sets `cpu1.pc = entry.unwrap()` exactly once. All other cpu1 state matches `XtensaLx7::reset()` (registers zero, WB=0, WS=1, PS as ROM-default). On real silicon the firmware's APP_CPU wrapper (linked into the ELF) sets up SP and jumps; we just start fetching at the entry.

### Shared-memory & atomics under quantum interleaving

Single-threaded quantum interleaving means that within a quantum, one core executes uninterruptible w.r.t. the other. Acquire/Release pairing is correct by construction on this simulator.

The firmware MUST still use Acquire/Release rather than Relaxed because:
1. Real silicon needs the ordering
2. We get decoder coverage of `L32AI` / `S32RI` in an end-to-end context
3. The same firmware ELF should be runnable on hardware with correct semantics

### Peripheral tick cadence

Today, `SystemBus::tick_peripherals()` advances peripherals by one tick per call. With quantum interleaving (N=256), peripheral state would drift relative to firmware's wall-clock expectation. Two acceptable implementations:

- (a) Add `tick_peripherals_n(n: usize)` that advances peripherals by N ticks atomically.
- (b) Call existing `tick_peripherals()` N times in the outer loop.

Plan task chooses one based on ergonomics and existing IRQ-collection semantics.

### Failure modes

- **Firmware never calls `start_app_core`** → `released` stays false; cpu1 parked; identical to Plan 4 behavior. ✓
- **Entry set to garbage** → cpu1 fetches bad addr, traps via existing illegal-instruction path; reported by sim. (Firmware bug, not sim bug.)
- **SYSTEM gate writes in unusual order** → controller checks all four conditions on every write; order doesn't matter.
- **Double bringup (`start_app_core` twice)** → entry overwritten; cpu1.pc re-set on next released-edge transition. Documented as "don't" rather than modeled as a hardware fault.

## Components & file plan

### New files

- `crates/core/src/system/core_controller.rs` — `CoreController` struct + accessors.
- `examples/esp32s3-dual-core-tmp102/` — firmware crate (single ELF; PRO reads TMP102 + writes `static AtomicI32`; APP reads it + drives GPIO2 via `CpuControl::start_app_core`).
- `examples/esp32s3-dual-core-tmp102/README.md` + `RUNBOOK.md`.
- `crates/core/tests/e2e_dual_core_tmp102.rs` — end-to-end: both `[PRO]` and `[APP]` stdout lines appear, GPIO2 toggles ≥ 1× within ~2B outer-loop iterations.
- `crates/core/tests/xtensa_dual_core_bringup.rs` — focused: asm fixture exercises the bringup register sequence; cpu1.pc lands at entry; negative case keeps cpu1 parked.
- `fixtures/xtensa-asm/appcpu_bringup.s` — minimal asm for the bringup test.

### Modified files

- `crates/core/src/cpu/xtensa_lx7.rs` — add `XtensaLx7::new_with_prid(prid: u32) -> Self`. Default `new()` keeps PRO_CPU PRID and existing reset behavior. Verify CCOUNT/INTENABLE/EPCn are already per-instance.
- `crates/core/src/peripherals/esp32s3/rom_thunks.rs:195` — replace NOP `ets_set_appcpu_boot_addr` body to capture `a2` into the shared `CoreController`.
- `crates/core/src/peripherals/esp32s3/system_stub.rs` — recognize writes to offset 0x18C; route bits 0/1/2 into `CoreController`; pass through to backing store. Other offsets unchanged.
- `crates/core/src/system/xtensa.rs` — `Esp32s3Wiring` gains `cpu1: XtensaLx7` and `core_controller: Arc<Mutex<CoreController>>`. `configure_xtensa_esp32s3` constructs both cores and the controller; passes the controller into `system_stub` and the rom thunk bank.
- `crates/core/src/multi_core.rs` — `step_all()` gains a quantum-size param (default 256); accepts `Option<Arc<Mutex<CoreController>>>` and skips cpu1 when not released.
- `crates/core/src/bus/mod.rs` — add `tick_peripherals_n(n: usize)` (or document calling `tick_peripherals()` N times — plan task picks).
- The simulation runner (CLI / e2e test harness) — wire cpu1 + controller into `MultiCoreMachine`; existing single-core entry points remain valid (cpu1 absent → single-core path).
- `crates/hw-oracle/src/lib.rs` — `OracleState` gains `appcpu_pc`, `appcpu_released`. `capture_sim_state` reads from `MultiCoreMachine` when present.
- `crates/hw-oracle/tests/oracles.rs` — add `appcpu_bringup_captures_entry` oracle.

### Untouched (load-bearing assumption)

- `intmatrix` per-CPU routing (already in tree from Plan 3)
- TMP102, GPIO, IO_MUX, USB_SERIAL_JTAG, SYSTIMER (used as-is from Plan 4)
- All existing single-core boot path

## Firmware shape (`examples/esp32s3-dual-core-tmp102/src/main.rs`)

```rust
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicI32, Ordering};
use esp_hal::{
    clock::ClockControl, cpu_control::{CpuControl, Stack},
    delay::Delay, gpio::Io, i2c::I2C, peripherals::Peripherals, prelude::*,
};

static SHARED_TEMP_CENTI_C: AtomicI32 = AtomicI32::new(0);
const THRESHOLD_CENTI_C: i32 = 25 * 100;     // 25.00 C
static mut APP_STACK: Stack<8192> = Stack::new();

#[entry]
fn main() -> ! {
    let peripherals = Peripherals::take();
    let system   = peripherals.SYSTEM.split();
    let _clocks  = ClockControl::boot_defaults(system.clock_control).freeze();
    let io       = Io::new(peripherals.GPIO, peripherals.IO_MUX);
    let led      = io.pins.gpio2.into_push_pull_output();
    let mut i2c  = I2C::new(peripherals.I2C0, io.pins.gpio4, io.pins.gpio5, 400.kHz());
    let mut cpu_ctrl = CpuControl::new(peripherals.CPU_CTRL);

    // Move LED handle into APP_CPU's closure
    let _guard = cpu_ctrl
        .start_app_core(unsafe { &mut APP_STACK }, move || app_main(led))
        .unwrap();

    let delay = Delay::new();
    loop {
        let mut buf = [0u8; 2];
        i2c.write_read(0x48, &[0x00], &mut buf).unwrap();
        let raw_u: u32 = ((buf[0] as u32) << 8) | (buf[1] as u32);
        let units_16: i32 = (raw_u >> 4) as i32;
        let centi_c: i32 = units_16 * 625 / 100;
        SHARED_TEMP_CENTI_C.store(centi_c, Ordering::Release);
        let int_part = centi_c / 100;
        let frac_part = centi_c.unsigned_abs() % 100;
        esp_println::println!("[PRO] T = {}.{:02} C", int_part, frac_part);
        delay.delay_millis(500);
    }
}

fn app_main(mut led: impl esp_hal::gpio::OutputPin) -> ! {
    let mut last = false;
    loop {
        let t = SHARED_TEMP_CENTI_C.load(Ordering::Acquire);
        let on = t > THRESHOLD_CENTI_C;
        if on != last {
            if on { led.set_high(); } else { led.set_low(); }
            esp_println::println!("[APP] LED={}", if on { "high" } else { "low" });
            last = on;
        }
    }
}
```

(Exact esp-hal call surface may shift to match the version Plan 4 pinned; plan task confirms.)

## Testing strategy

| Level | File | What it proves |
|---|---|---|
| Unit | `core_controller.rs` (inline) | State machine: entry alone ≠ released; gates without entry ≠ released; all four conditions ≠ released. |
| Unit | `cpu/xtensa_lx7.rs` (inline) | `new_with_prid()` sets PRID 0xCDCD vs 0xABAB; default `new()` unchanged. |
| Unit | `system_stub.rs` (inline) | Writes at offset 0x18C route bits 0/1/2 to `CoreController`; other offsets pass through. |
| Integration | `xtensa_dual_core_bringup.rs` | Bringup asm fixture: cpu1.pc lands at entry exactly once; single-core fixture leaves cpu1 parked forever. |
| E2E | `e2e_dual_core_tmp102.rs` | Headline: ≥ 5 `[PRO]` lines AND ≥ 1 `[APP]` line AND GPIO2 transitions ≥ 1× within ~2B outer-loop iterations. |
| Oracle | `oracles.rs` | `appcpu_bringup_captures_entry` — sim runs bringup asm; `OracleState` reports `appcpu_pc == entry`, `appcpu_released == true`. Hardware diff deferred (no custom hw fixture in oracle harness today). |

### Observability

- Both cores' stdout differentiated by `[PRO]` / `[APP]` prefix
- `tracing::info!` at three milestones: `appcpu_entry_set`, `appcpu_released`, `appcpu_first_fetch_pc=0x…`
- GPIO observer on pin 2 records every transition with sim-cycle timestamp (existing infra)

## Success criteria

1. All existing tests still pass (single-core back-compat).
2. New unit + integration + e2e tests green.
3. Firmware example builds with the same esp-hal release Plan 4 pinned, boots end-to-end on the simulator.
4. README + RUNBOOK explain what the demo claims and how to reproduce on hardware.
5. CI green; no admin merge.

## Explicit non-goals (calling out so they don't get smuggled in)

- Hardware-validated cycle counts on dual-core
- APP_CPU taking interrupts
- Embassy-on-dual-core
- Inter-core software IRQs (`FROM_CPU_INTR_n`)
- Cache modeling / region-aware load latency
- Full `RUNSTALL` mid-execution semantics
