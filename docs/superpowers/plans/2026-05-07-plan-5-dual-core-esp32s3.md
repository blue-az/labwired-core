# Plan 5 — Dual-Core ESP32-S3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring up ESP32-S3's APP_CPU under faithful esp-hal bringup semantics and ship a producer/consumer demo where PRO reads TMP102 + writes a shared `AtomicI32`, and APP reads it + drives GPIO2 on a temperature crossing.

**Architecture:** A new `CoreController` struct gates APP_CPU until firmware sets the entry address (via `ets_set_appcpu_boot_addr` ROM thunk) and opens the SYSTEM `CORE_1_CONTROL_0` gates (offset 0x18C, bits 0/1/2). The simulation outer loop steps PRO_CPU every iteration and steps APP_CPU only when `is_app_cpu_released()` is true. Shared memory + atomics work by construction under single-threaded interleaving (`L32AI`/`S32RI` are already decoded). Single-core firmware behavior is byte-identical because no code path runs cpu1 unless firmware explicitly releases it.

**Tech Stack:** Rust 2021, esp-hal 1.1 (Xtensa toolchain via `cargo +esp`), Xtensa LX7 (existing CPU model), `Arc<Mutex<CoreController>>` for shared state, the existing `RomThunkBank` + `SystemStub` peripheral patterns.

**Spec:** `docs/superpowers/specs/2026-05-07-plan-5-dual-core-esp32s3-design.md`

---

## File Structure (locked in)

**New files:**
- `crates/core/src/system/core_controller.rs` — `CoreController` struct + state machine.
- `examples/esp32s3-dual-core-tmp102/` — firmware crate (mirrors `examples/esp32s3-i2c-tmp102/` shape: `Cargo.toml`, `.cargo/config.toml`, `rust-toolchain.toml`, `build.rs`, `src/main.rs`).
- `examples/esp32s3-dual-core-tmp102/README.md` + `RUNBOOK.md`.
- `crates/core/tests/xtensa_dual_core_bringup.rs` — focused integration test for the bringup register sequence (no firmware needed).
- `crates/core/tests/e2e_dual_core_tmp102.rs` — headline end-to-end test against compiled esp-hal firmware.
- `fixtures/xtensa-asm/appcpu_bringup.s` — minimal asm fixture exercising the bringup sequence.

**Modified files:**
- `crates/core/src/system/mod.rs` — re-export `CoreController`.
- `crates/core/src/lib.rs` — extend `Bus` trait with default `fn core_controller(&self) -> Option<Arc<Mutex<CoreController>>> { None }`.
- `crates/core/src/bus/mod.rs` — `SystemBus` gains `core_controller: Option<Arc<Mutex<CoreController>>>` field + impls.
- `crates/core/src/cpu/xtensa_sr.rs` — `XtensaSrFile::with_prid(prid: u32)` constructor.
- `crates/core/src/cpu/xtensa_lx7.rs` — `XtensaLx7::new_with_prid(prid: u32)` constructor.
- `crates/core/src/peripherals/esp32s3/rom_thunks.rs` — `ets_set_appcpu_boot_addr` reads `a2` (entry) and writes it into `bus.core_controller()`.
- `crates/core/src/peripherals/esp32s3/system_stub.rs` — `SystemStub::with_core_controller(...)` constructor; writes to offset 0x18C route bits 0/1/2 into the controller AND fall through to backing store.
- `crates/core/src/system/xtensa.rs` — `Esp32s3Wiring` gains `cpu1` and `core_controller`. `configure_xtensa_esp32s3` constructs both, threads the controller into `SystemBus`, the rom thunk bank's containing peripheral, and `SystemStub`.
- `crates/hw-oracle/src/lib.rs` — `OracleState` gains `appcpu_pc: u32` and `appcpu_released: bool`; `capture_sim_state` reads from the wiring.
- `crates/hw-oracle/tests/oracles.rs` — `appcpu_bringup_captures_entry` oracle case.

**Untouched:**
- `intmatrix`, TMP102, GPIO, IO_MUX, USB_SERIAL_JTAG, SYSTIMER (used as-is).
- `MultiCoreMachine` in `crates/core/src/multi_core.rs` — leave alone for now. The dual-core test runner inlines its own outer loop (matching the Plan 4 e2e pattern of `cpu.step()` + `bus.tick_peripherals_with_costs()`). Keeps the change footprint surgical and avoids refactoring an unused-in-this-codepath abstraction.

---

## Task 1: `CoreController` scaffolding

**Files:**
- Create: `crates/core/src/system/core_controller.rs`
- Modify: `crates/core/src/system/mod.rs`

- [ ] **Step 1: Create the file with the struct and unit tests (test-first)**

```rust
// crates/core/src/system/core_controller.rs
// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! APP_CPU bringup gate.
//!
//! Tracks the four conditions ESP32-S3 firmware satisfies to release the
//! second core: an entry address (set via `ets_set_appcpu_boot_addr`), and
//! three bits in `SYSTEM_CORE_1_CONTROL_0_REG` (0x600C_018C):
//! `RUNSTALL` (bit 1), `RESETTING` (bit 2), `CLKGATE_EN` (bit 0).
//!
//! Shared between the rom-thunk that captures the entry and the
//! `SystemStub` that watches the SYSTEM register writes via
//! `Arc<Mutex<CoreController>>`.

#[derive(Debug, Default, Clone, Copy)]
pub struct CoreController {
    pub appcpu_entry: Option<u32>,
    pub runstall: bool,
    pub reset_en: bool,
    pub clkgate_en: bool,
}

impl CoreController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_entry(&mut self, addr: u32) {
        self.appcpu_entry = Some(addr);
    }

    pub fn entry(&self) -> Option<u32> {
        self.appcpu_entry
    }

    pub fn set_runstall(&mut self, on: bool) {
        self.runstall = on;
    }

    pub fn set_reset_en(&mut self, on: bool) {
        self.reset_en = on;
    }

    pub fn set_clkgate_en(&mut self, on: bool) {
        self.clkgate_en = on;
    }

    /// True once the firmware has set an entry AND opened all three gates.
    /// On real silicon this corresponds to APP_CPU starting to fetch.
    pub fn is_app_cpu_released(&self) -> bool {
        self.appcpu_entry.is_some() && !self.runstall && !self.reset_en && self.clkgate_en
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_not_released() {
        let c = CoreController::new();
        assert!(!c.is_app_cpu_released());
        assert_eq!(c.entry(), None);
    }

    #[test]
    fn entry_alone_does_not_release() {
        let mut c = CoreController::new();
        c.set_entry(0x4080_1234);
        assert!(!c.is_app_cpu_released());
    }

    #[test]
    fn gates_alone_do_not_release() {
        let mut c = CoreController::new();
        c.set_clkgate_en(true);
        c.set_reset_en(false);
        c.set_runstall(false);
        assert!(!c.is_app_cpu_released());
    }

    #[test]
    fn entry_plus_open_gates_releases() {
        let mut c = CoreController::new();
        c.set_entry(0x4080_1234);
        c.set_clkgate_en(true);
        // reset_en / runstall default to false (their "released" values)
        assert!(c.is_app_cpu_released());
        assert_eq!(c.entry(), Some(0x4080_1234));
    }

    #[test]
    fn runstall_blocks_release() {
        let mut c = CoreController::new();
        c.set_entry(0x4080_1234);
        c.set_clkgate_en(true);
        c.set_runstall(true);
        assert!(!c.is_app_cpu_released());
        c.set_runstall(false);
        assert!(c.is_app_cpu_released());
    }

    #[test]
    fn reset_en_blocks_release() {
        let mut c = CoreController::new();
        c.set_entry(0x4080_1234);
        c.set_clkgate_en(true);
        c.set_reset_en(true);
        assert!(!c.is_app_cpu_released());
        c.set_reset_en(false);
        assert!(c.is_app_cpu_released());
    }
}
```

- [ ] **Step 2: Add module to `system/mod.rs`**

Open `crates/core/src/system/mod.rs` and add (in the existing module declarations, alphabetically):

```rust
pub mod core_controller;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p labwired-core --lib system::core_controller`
Expected: 6 tests pass (all five listed above + any compile-only checks).

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/system/core_controller.rs crates/core/src/system/mod.rs
git commit -m "feat(core): CoreController struct + state machine for APP_CPU bringup gate"
```

---

## Task 2: `Bus` trait + `SystemBus` carries the controller handle

**Files:**
- Modify: `crates/core/src/lib.rs` (add default trait method `core_controller`)
- Modify: `crates/core/src/bus/mod.rs` (`SystemBus` field + accessor + impl)

- [ ] **Step 1: Write the failing test in `bus/mod.rs`**

Append to the existing `#[cfg(test)] mod tests` block in `crates/core/src/bus/mod.rs` (or add the block at the end if absent). The test verifies a fresh `SystemBus` has no controller and that one set via `set_core_controller` round-trips.

```rust
#[test]
fn core_controller_default_none_and_setter_round_trips() {
    use crate::system::core_controller::CoreController;
    use std::sync::{Arc, Mutex};

    let mut bus = SystemBus::new();
    assert!(bus.core_controller().is_none(), "fresh bus has no controller");

    let handle = Arc::new(Mutex::new(CoreController::new()));
    bus.set_core_controller(handle.clone());

    let got = bus.core_controller().expect("bus now exposes controller");
    got.lock().unwrap().set_entry(0xDEAD_BEEF);
    assert_eq!(handle.lock().unwrap().entry(), Some(0xDEAD_BEEF));
}
```

- [ ] **Step 2: Run the test to verify it fails (compile error or assertion)**

Run: `cargo test -p labwired-core --lib bus::tests::core_controller_default_none_and_setter_round_trips`
Expected: compile failure — `SystemBus::core_controller` and `set_core_controller` don't exist.

- [ ] **Step 3: Add the trait default in `lib.rs`**

In `crates/core/src/lib.rs`, find the `pub trait Bus` block (look for `fn get_rom_thunk`). Add this default-implementing method just below `get_rom_thunk`:

```rust
    /// Plan 5: APP_CPU bringup controller, when the bus is wired for an
    /// ESP32-S3 dual-core simulation. Default returns None; only
    /// `SystemBus::set_core_controller(...)` populates it.
    fn core_controller(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<crate::system::core_controller::CoreController>>>
    {
        None
    }
```

- [ ] **Step 4: Add the field + accessors to `SystemBus`**

In `crates/core/src/bus/mod.rs`, find the `pub struct SystemBus { ... }` definition and add the field:

```rust
    /// Plan 5: optional shared APP_CPU bringup controller. Populated by
    /// `configure_xtensa_esp32s3` when the bus is wired for ESP32-S3
    /// dual-core simulation. Cloned to the rom-thunk dispatch surface and
    /// to `SystemStub` so all three (write-watcher, thunk, runner) see
    /// the same state.
    pub core_controller:
        Option<std::sync::Arc<std::sync::Mutex<crate::system::core_controller::CoreController>>>,
```

In `SystemBus::new()` add the field initializer (`core_controller: None,`).

Add a setter and override the trait method in the `SystemBus` impl (not the trait impl):

```rust
impl SystemBus {
    // ...existing methods...

    pub fn set_core_controller(
        &mut self,
        ctrl: std::sync::Arc<std::sync::Mutex<crate::system::core_controller::CoreController>>,
    ) {
        self.core_controller = Some(ctrl);
    }
}
```

In the existing `impl Bus for SystemBus` block, add:

```rust
    fn core_controller(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<crate::system::core_controller::CoreController>>>
    {
        self.core_controller.clone()
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p labwired-core --lib bus::tests::core_controller_default_none_and_setter_round_trips`
Expected: PASS.

- [ ] **Step 6: Verify the rest of the workspace still builds**

Run: `cargo check --workspace --all-targets`
Expected: no errors. (Other `Bus` impls inherit the default `None`.)

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/lib.rs crates/core/src/bus/mod.rs
git commit -m "feat(core): Bus carries optional CoreController handle for APP_CPU bringup"
```

---

## Task 3: `XtensaLx7::new_with_prid`

**Files:**
- Modify: `crates/core/src/cpu/xtensa_sr.rs`
- Modify: `crates/core/src/cpu/xtensa_lx7.rs`

- [ ] **Step 1: Write the failing test in `xtensa_lx7.rs`**

Find the existing `#[cfg(test)] mod tests` block at the bottom of `crates/core/src/cpu/xtensa_lx7.rs` (or add one). Add:

```rust
    #[test]
    fn new_with_prid_sets_supplied_value() {
        let cpu = XtensaLx7::new_with_prid(0xABAB);
        // PRID is SR id 235 (0xEB).
        assert_eq!(cpu.sr.read(235), 0xABAB);
    }

    #[test]
    fn default_new_keeps_pro_cpu_prid() {
        let cpu = XtensaLx7::new();
        assert_eq!(cpu.sr.read(235), 0xCDCD); // PRID_RESET_VALUE
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p labwired-core --lib cpu::xtensa_lx7::tests::new_with_prid_sets_supplied_value`
Expected: compile error — `new_with_prid` doesn't exist.

- [ ] **Step 3: Add `XtensaSrFile::with_prid` constructor**

In `crates/core/src/cpu/xtensa_sr.rs`, find the `impl XtensaSrFile` block and add (right after `pub fn new()`):

```rust
    /// Construct an SR file with `PRID` set to a custom value.
    /// Other reset values match `Self::new()`. Used to distinguish PRO_CPU
    /// (0xCDCD) from APP_CPU (0xABAB) per real ESP32-S3 silicon.
    pub fn with_prid(prid: u32) -> Self {
        let mut s = Self::new();
        s.storage[IDX_PRID] = prid;
        s
    }
```

- [ ] **Step 4: Add `XtensaLx7::new_with_prid` constructor**

In `crates/core/src/cpu/xtensa_lx7.rs`, find `impl XtensaLx7` (right after `pub fn new()`). Add:

```rust
    /// Construct a CPU with a custom `PRID`. Identical to `new()` except for
    /// the `PRID` SR. Use 0xCDCD for PRO_CPU (default) and 0xABAB for APP_CPU.
    pub fn new_with_prid(prid: u32) -> Self {
        Self {
            regs: ArFile::new(),
            ps: Ps::from_raw(0x1F),
            sr: XtensaSrFile::with_prid(prid),
            ur: [0u32; 256],
            pc: 0x4000_0400,
        }
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p labwired-core --lib cpu::xtensa_lx7::tests::new_with_prid_sets_supplied_value cpu::xtensa_lx7::tests::default_new_keeps_pro_cpu_prid`
Expected: both PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/cpu/xtensa_sr.rs crates/core/src/cpu/xtensa_lx7.rs
git commit -m "feat(xtensa): XtensaLx7::new_with_prid for per-core PRID (PRO 0xCDCD, APP 0xABAB)"
```

---

## Task 4: `ets_set_appcpu_boot_addr` captures entry into the controller

**Files:**
- Modify: `crates/core/src/peripherals/esp32s3/rom_thunks.rs:195-200`

The thunk receives `&mut dyn Bus` and the C-ABI first argument lives in the post-CALL window's a2 (i.e., `regs.read_logical(callinc * 4 + 2)`). The thunk reads it and writes it into `bus.core_controller()`.

- [ ] **Step 1: Write the failing unit test in `rom_thunks.rs`**

Append to the existing `#[cfg(test)] mod tests` block at the bottom of `crates/core/src/peripherals/esp32s3/rom_thunks.rs`:

```rust
    #[test]
    fn ets_set_appcpu_boot_addr_captures_entry_into_controller() {
        use crate::bus::SystemBus;
        use crate::system::core_controller::CoreController;
        use crate::Bus;
        use std::sync::{Arc, Mutex};

        let mut cpu = XtensaLx7::new();
        let mut bus = SystemBus::new();
        let ctrl = Arc::new(Mutex::new(CoreController::new()));
        bus.set_core_controller(ctrl.clone());

        // Simulate post-CALL4 frame: callinc=1, a6 (= a[1*4 + 2]) holds the entry.
        cpu.ps = crate::cpu::xtensa_regs::Ps::from_raw(0x4_001F); // CALLINC=1
        cpu.regs.write_logical(4, 0x4080_1000); // saved a0 (return PC encoding)
        cpu.regs.write_logical(6, 0x4080_2468); // a2 of callee = entry

        super::ets_set_appcpu_boot_addr(&mut cpu, &mut bus).unwrap();

        assert_eq!(
            ctrl.lock().unwrap().entry(),
            Some(0x4080_2468),
            "thunk must capture entry into shared controller"
        );
    }

    #[test]
    fn ets_set_appcpu_boot_addr_no_controller_is_safe_nop() {
        // Without a controller wired, the thunk must still return cleanly
        // (preserves single-core back-compat for non-S3 wiring paths).
        let mut cpu = XtensaLx7::new();
        let mut bus = crate::bus::SystemBus::new();
        cpu.ps = crate::cpu::xtensa_regs::Ps::from_raw(0x4_001F);
        cpu.regs.write_logical(4, 0x4080_1000);
        cpu.regs.write_logical(6, 0x4080_2468);
        super::ets_set_appcpu_boot_addr(&mut cpu, &mut bus).unwrap();
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p labwired-core --lib peripherals::esp32s3::rom_thunks::tests::ets_set_appcpu_boot_addr`
Expected: first test FAILs (controller stays empty); second PASSes by accident (current NOP returns Ok).

- [ ] **Step 3: Replace the thunk body**

In `crates/core/src/peripherals/esp32s3/rom_thunks.rs`, replace the existing function (line ~195):

```rust
/// `ets_set_appcpu_boot_addr(addr: u32) -> u32` — captures `addr` into the
/// shared `CoreController` so that the dual-core runner can release APP_CPU
/// once SYSTEM gates open. Returns 0 (success) like the BROM symbol.
pub fn ets_set_appcpu_boot_addr(cpu: &mut XtensaLx7, bus: &mut dyn Bus) -> SimResult<()> {
    // C-ABI first arg lives at a[CALLINC*4 + 2] in the callee's frame.
    let n = cpu.ps.callinc() * 4;
    let addr = cpu.regs.read_logical(n + 2);

    if let Some(ctrl) = bus.core_controller() {
        ctrl.lock().unwrap().set_entry(addr);
    } else {
        tracing::trace!(
            "ets_set_appcpu_boot_addr: addr=0x{addr:08x} but no CoreController wired \
             (single-core configuration?); ignoring."
        );
    }

    RomThunkBank::return_with(cpu, 0);
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify both pass**

Run: `cargo test -p labwired-core --lib peripherals::esp32s3::rom_thunks::tests::ets_set_appcpu_boot_addr`
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/peripherals/esp32s3/rom_thunks.rs
git commit -m "feat(esp32s3): ets_set_appcpu_boot_addr captures entry into CoreController"
```

---

## Task 5: `SystemStub` routes 0x18C writes into the controller

The SYSTEM register `CORE_1_CONTROL_0_REG` lives at offset 0x18C from SYSTEM base 0x600C_0000:
- bit 0 = `CONTROL_CORE_1_CLKGATE_EN`
- bit 1 = `CONTROL_CORE_1_RUNSTALL`
- bit 2 = `CONTROL_CORE_1_RESETTING` (named `reset_en` in `CoreController` for brevity)

`SystemStub` will optionally hold an `Arc<Mutex<CoreController>>`. Writes to offset 0x18C update the controller AND fall through to the existing word-store (so reads round-trip the firmware's last written value).

**Files:**
- Modify: `crates/core/src/peripherals/esp32s3/system_stub.rs`

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` block in `crates/core/src/peripherals/esp32s3/system_stub.rs`:

```rust
    #[test]
    fn system_stub_routes_core_1_control_0_to_controller() {
        use crate::system::core_controller::CoreController;
        use std::sync::{Arc, Mutex};

        let ctrl = Arc::new(Mutex::new(CoreController::new()));
        let mut s = SystemStub::with_core_controller(ctrl.clone());

        // Write CLKGATE_EN=1 (bit 0) at offset 0x18C, byte 0.
        s.write(0x18C, 0x01).unwrap();
        assert!(ctrl.lock().unwrap().clkgate_en);
        assert!(!ctrl.lock().unwrap().runstall);
        assert!(!ctrl.lock().unwrap().reset_en);

        // Pulse RESETTING (bit 2) high, then low — keep clkgate set.
        s.write(0x18C, 0x05).unwrap(); // bits 0|2
        assert!(ctrl.lock().unwrap().reset_en);

        s.write(0x18C, 0x01).unwrap(); // back to CLKGATE only
        assert!(!ctrl.lock().unwrap().reset_en);

        // Read-back: backing store should reflect last write.
        assert_eq!(s.read(0x18C).unwrap(), 0x01);
    }

    #[test]
    fn system_stub_other_offsets_unaffected_by_controller_routing() {
        use crate::system::core_controller::CoreController;
        use std::sync::{Arc, Mutex};

        let ctrl = Arc::new(Mutex::new(CoreController::new()));
        let mut s = SystemStub::with_core_controller(ctrl.clone());
        s.write(0x10, 0xAB).unwrap();
        assert_eq!(s.read(0x10).unwrap(), 0xAB);
        assert!(!ctrl.lock().unwrap().clkgate_en);
        assert_eq!(ctrl.lock().unwrap().entry(), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p labwired-core --lib peripherals::esp32s3::system_stub::tests::system_stub_routes_core_1_control_0_to_controller`
Expected: compile error — `with_core_controller` doesn't exist.

- [ ] **Step 3: Update `SystemStub` to optionally hold the controller**

In `crates/core/src/peripherals/esp32s3/system_stub.rs`:

Add the field (next to `unwritten_read`):

```rust
    /// Optional shared APP_CPU bringup controller. When present, writes to
    /// offset 0x18C (`CORE_1_CONTROL_0_REG`) route bits 0/1/2 into the
    /// controller AND fall through to the backing store.
    core_controller:
        Option<std::sync::Arc<std::sync::Mutex<crate::system::core_controller::CoreController>>>,
```

Update the existing `new()` and `with_unwritten_ones()` constructors to initialise the new field to `None`:

```rust
    pub fn new() -> Self {
        Self {
            words: HashMap::new(),
            unwritten_read: 0,
            core_controller: None,
        }
    }

    pub fn with_unwritten_ones() -> Self {
        Self {
            words: HashMap::new(),
            unwritten_read: u32::MAX,
            core_controller: None,
        }
    }
```

Add a new constructor:

```rust
    /// Variant that wires a shared `CoreController`. Writes to
    /// `CORE_1_CONTROL_0_REG` (offset 0x18C) update the controller in
    /// addition to the backing store.
    pub fn with_core_controller(
        ctrl: std::sync::Arc<std::sync::Mutex<crate::system::core_controller::CoreController>>,
    ) -> Self {
        Self {
            words: HashMap::new(),
            unwritten_read: 0,
            core_controller: Some(ctrl),
        }
    }
```

Update the `Peripheral::write` impl to detect writes to offset 0x18C (any of bytes 0x18C..0x18F) and route the resulting word into the controller after updating the backing store:

```rust
    fn write(&mut self, offset: u64, value: u8) -> SimResult<()> {
        let word_off = offset & !3;
        let byte_off = (offset & 3) * 8;
        let entry = self.words.entry(word_off).or_insert(self.unwritten_read);
        *entry &= !(0xFFu32 << byte_off);
        *entry |= (value as u32) << byte_off;
        let new_word = *entry;

        // CORE_1_CONTROL_0_REG (ESP32-S3 TRM §11). Three bits of interest:
        //   bit 0 = CLKGATE_EN, bit 1 = RUNSTALL, bit 2 = RESETTING.
        if word_off == 0x18C {
            if let Some(ctrl) = &self.core_controller {
                let mut c = ctrl.lock().unwrap();
                c.set_clkgate_en((new_word & 0x1) != 0);
                c.set_runstall((new_word & 0x2) != 0);
                c.set_reset_en((new_word & 0x4) != 0);
            }
        }

        Ok(())
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p labwired-core --lib peripherals::esp32s3::system_stub::tests`
Expected: all tests in this module pass (including the two new ones).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/peripherals/esp32s3/system_stub.rs
git commit -m "feat(esp32s3): SystemStub routes CORE_1_CONTROL_0 writes into CoreController"
```

---

## Task 6: Wire dual-core into `Esp32s3Wiring`

**Files:**
- Modify: `crates/core/src/system/xtensa.rs`

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` block at the bottom of `crates/core/src/system/xtensa.rs`:

```rust
    #[test]
    fn wiring_exposes_cpu1_with_app_prid() {
        let mut bus = SystemBus::new();
        let wiring = configure_xtensa_esp32s3(&mut bus, &Esp32s3Opts::default());
        // PRO_CPU PRID is 0xCDCD; APP_CPU PRID is 0xABAB.
        assert_eq!(wiring.cpu.sr.read(235), 0xCDCD);
        assert_eq!(wiring.cpu1.sr.read(235), 0xABAB);
    }

    #[test]
    fn wiring_shares_one_core_controller_across_bus_and_stub() {
        let mut bus = SystemBus::new();
        let wiring = configure_xtensa_esp32s3(&mut bus, &Esp32s3Opts::default());

        // Bus exposes the same controller handle as the wiring.
        let bus_ctrl = bus.core_controller().expect("bus carries controller");
        assert!(std::sync::Arc::ptr_eq(&bus_ctrl, &wiring.core_controller));

        // A write to SYSTEM CORE_1_CONTROL_0 propagates to the controller.
        bus.write_u8(0x600C_018C, 0x01).unwrap(); // CLKGATE_EN
        assert!(wiring.core_controller.lock().unwrap().clkgate_en);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p labwired-core --lib system::xtensa::tests::wiring_exposes_cpu1_with_app_prid`
Expected: compile error — `Esp32s3Wiring` has no `cpu1` or `core_controller` field.

- [ ] **Step 3: Update `Esp32s3Wiring` struct**

In `crates/core/src/system/xtensa.rs`, find the `pub struct Esp32s3Wiring` and add the two fields:

```rust
pub struct Esp32s3Wiring {
    pub cpu: XtensaLx7,
    /// APP_CPU. Constructed at boot but not stepped until
    /// `core_controller.is_app_cpu_released()` is true.
    pub cpu1: XtensaLx7,
    /// Shared bringup controller. Cloned into the bus and into the
    /// SYSTEM stub so all writes converge to the same state.
    pub core_controller: Arc<Mutex<crate::system::core_controller::CoreController>>,
    pub icache_backing: Arc<Mutex<Vec<u8>>>,
    pub dcache_backing: Arc<Mutex<Vec<u8>>>,
}
```

(`Arc`/`Mutex` are already imported at the top of the file.)

- [ ] **Step 4: Update `configure_xtensa_esp32s3`**

In the same file, find `configure_xtensa_esp32s3`. Make these changes:

(a) At the top, after `bus.peripherals.clear();` and `bus.bit_band_enabled = false;`, construct the controller and wire it onto the bus:

```rust
    // Plan 5: shared APP_CPU bringup controller. Threaded into the
    // SYSTEM stub (which watches CORE_1_CONTROL_0 writes) and onto
    // the bus (so the ets_set_appcpu_boot_addr ROM thunk can capture
    // the entry address).
    let core_controller = Arc::new(Mutex::new(
        crate::system::core_controller::CoreController::new(),
    ));
    bus.set_core_controller(core_controller.clone());
```

(b) Replace the existing SystemStub registration for the `"system"` peripheral so it uses the controller-aware constructor:

Find:

```rust
    bus.add_peripheral(
        "system",
        0x600C_0000,
        0x1_0000,
        None,
        Box::new(SystemStub::new()),
    );
```

Replace with:

```rust
    bus.add_peripheral(
        "system",
        0x600C_0000,
        0x1_0000,
        None,
        Box::new(SystemStub::with_core_controller(core_controller.clone())),
    );
```

(c) At the bottom, replace the `Esp32s3Wiring { ... }` construction:

```rust
    let mut cpu = XtensaLx7::new();
    cpu.reset(bus).expect("xtensa reset");

    let mut cpu1 = XtensaLx7::new_with_prid(0xABAB);
    cpu1.reset(bus).expect("xtensa app_cpu reset");

    Esp32s3Wiring {
        cpu,
        cpu1,
        core_controller,
        icache_backing,
        dcache_backing,
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p labwired-core --lib system::xtensa::tests`
Expected: existing tests still pass + two new ones pass.

- [ ] **Step 6: Verify the workspace still builds (existing e2e tests use `wiring.cpu` only — they are unaffected)**

Run: `cargo check --workspace --all-targets`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/system/xtensa.rs
git commit -m "feat(esp32s3): wire cpu1 + shared CoreController in Esp32s3Wiring"
```

---

## Task 7: Bringup integration test (asm fixture)

This is a focused test that exercises the full bringup register sequence without firmware. It's the "I'm changing nothing else" canary that locks the wiring in place.

**Files:**
- Create: `fixtures/xtensa-asm/appcpu_bringup.s`
- Modify: `fixtures/xtensa-asm/Makefile` (only if the existing `%.elf: %.s` generic rule isn't already there from Plan 4 — it should be; verify and add only if missing)
- Create: `crates/core/tests/xtensa_dual_core_bringup.rs`

The fixture has PRO_CPU pretend to be esp-hal's CpuControl::start_app_core: it writes to `ets_set_appcpu_boot_addr` via the BREAK 1, 14 thunk-call mechanism, then writes the SYSTEM gate, then loops forever. The test runs PRO until the controller reports released, then verifies state.

- [ ] **Step 1: Create the asm fixture**

Create `fixtures/xtensa-asm/appcpu_bringup.s`:

```
# Plan 5: APP_CPU bringup register sequence.
# PRO_CPU: set entry via ets_set_appcpu_boot_addr ROM thunk, then open SYSTEM gates.

    .literal_position
APP_ENTRY_LIT:   .word 0x40080123
SYS_CTRL_LIT:    .word 0x600C018C
ETS_SET_LIT:     .word 0x40000720    # ets_set_appcpu_boot_addr ROM symbol

    .text
    .global _start
_start:
    # Provide a minimal valid frame for CALL4 (callinc=1).
    movi    a1, 0x3FCDFF00     # SP

    # ---- Call ets_set_appcpu_boot_addr(0x40080123) via CALL4 ----
    l32r    a8, ETS_SET_LIT
    l32r    a10, APP_ENTRY_LIT  # callee a2 = a10 (CALL4 rotates +4)
    callx4  a8

    # ---- Write SYSTEM_CORE_1_CONTROL_0_REG = bit 0 (CLKGATE_EN=1) ----
    l32r    a2, SYS_CTRL_LIT
    movi    a3, 1
    s32i    a3, a2, 0

    # ---- Park PRO_CPU forever ----
park:
    j       park
```

- [ ] **Step 2: Verify Plan 4's `%.elf: %.s` Makefile rule still exists**

Run: `grep -n '%.elf: %.s' fixtures/xtensa-asm/Makefile`
Expected: a generic rule line. If absent, append to the Makefile:

```make
%.elf: %.s
	xtensa-esp32s3-elf-gcc -nostdlib -Wl,-Ttext=0x40800000 $< -o $@
```

- [ ] **Step 3: Build the fixture binary**

Run: `make -C fixtures/xtensa-asm appcpu_bringup.elf`
Expected: ELF produced. If the toolchain isn't installed, mark this fixture as build-time-required-when-running-the-test (we'll commit the .elf as Plan 4 did with `!*.elf` in `.gitignore`).

- [ ] **Step 4: Write the failing integration test**

Create `crates/core/tests/xtensa_dual_core_bringup.rs`:

```rust
// LabWired - Plan 5: dual-core bringup integration test.
// Drives PRO_CPU through the esp-hal-style CpuControl::start_app_core
// register sequence (ets_set_appcpu_boot_addr + SYSTEM CORE_1_CONTROL_0)
// and asserts the CoreController flips to released exactly once.

use labwired_core::boot::esp32s3::{fast_boot, BootOpts};
use labwired_core::bus::SystemBus;
use labwired_core::system::xtensa::{configure_xtensa_esp32s3, Esp32s3Opts};
use labwired_core::{Bus, Cpu, SimulationError};
use std::path::PathBuf;

fn fixture_elf() -> PathBuf {
    PathBuf::from("../../fixtures/xtensa-asm/appcpu_bringup.elf")
}

#[test]
fn bringup_releases_app_cpu_after_full_sequence() {
    let elf_bytes = std::fs::read(fixture_elf()).expect("appcpu_bringup.elf");

    let mut bus = SystemBus::new();
    let wiring = configure_xtensa_esp32s3(&mut bus, &Esp32s3Opts::default());
    let icache_backing = wiring.icache_backing.clone();
    let dcache_backing = wiring.dcache_backing.clone();
    let core_controller = wiring.core_controller.clone();
    let mut cpu = wiring.cpu;

    fast_boot(
        &elf_bytes,
        &mut bus,
        &mut cpu,
        &BootOpts {
            stack_top_fallback: 0x3FCD_FFF0,
            icache_backing: Some(icache_backing),
            dcache_backing: Some(dcache_backing),
        },
    )
    .expect("fast_boot");

    let observers: Vec<std::sync::Arc<dyn labwired_core::SimulationObserver>> = Vec::new();
    let cfg = labwired_core::SimulationConfig::default();

    // The fixture takes well under 1k instructions to reach the park loop.
    for _ in 0..100_000 {
        match cpu.step(&mut bus, &observers, &cfg) {
            Ok(()) => {}
            Err(SimulationError::BreakpointHit(_)) => break,
            Err(e) => panic!("PRO step error at 0x{:08x}: {e}", cpu.get_pc()),
        }
        let _ = bus.tick_peripherals_with_costs();
        if core_controller.lock().unwrap().is_app_cpu_released() {
            break;
        }
    }

    let c = core_controller.lock().unwrap();
    assert_eq!(c.entry(), Some(0x4008_0123), "entry captured by ROM thunk");
    assert!(c.clkgate_en, "CLKGATE_EN set by SYSTEM write");
    assert!(!c.runstall, "RUNSTALL stays clear (esp-hal default)");
    assert!(!c.reset_en, "RESETTING stays clear (esp-hal default)");
    assert!(c.is_app_cpu_released(), "all four gates open → released");
}

#[test]
fn no_bringup_keeps_app_cpu_parked() {
    // A bus that's never touched by firmware: cpu1 must remain unreleased.
    let mut bus = SystemBus::new();
    let wiring = configure_xtensa_esp32s3(&mut bus, &Esp32s3Opts::default());
    assert!(!wiring.core_controller.lock().unwrap().is_app_cpu_released());
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p labwired-core --test xtensa_dual_core_bringup`
Expected: both tests PASS.

- [ ] **Step 6: Force-add the .elf (it's covered by `!*.elf` in `fixtures/xtensa-asm/.gitignore`)**

```bash
git add -f fixtures/xtensa-asm/appcpu_bringup.s fixtures/xtensa-asm/appcpu_bringup.elf
git add crates/core/tests/xtensa_dual_core_bringup.rs
git commit -m "test(plan-5): bringup integration test exercises full register sequence"
```

---

## Task 8: HW-oracle bank entry for bringup

The oracle harness diffs `OracleState` between sim and hw runs. We add fields for APP_CPU PC + released flag. Hardware-side comparison is deferred (no custom hw fixture in oracle harness yet); the new oracle case just locks down sim state under the same fixture used in Task 7.

**Files:**
- Modify: `crates/hw-oracle/src/lib.rs`
- Modify: `crates/hw-oracle/tests/oracles.rs`

- [ ] **Step 1: Locate the `OracleState` struct definition**

Run: `grep -n "pub struct OracleState\|appcpu" crates/hw-oracle/src/lib.rs | head -10`

- [ ] **Step 2: Add fields to `OracleState`**

In the `OracleState` struct add (immediately after the `pc: u32` field, or alongside other CPU-state fields):

```rust
    /// Plan 5: APP_CPU program counter (0 if not released yet).
    #[serde(default)]
    pub appcpu_pc: u32,
    /// Plan 5: true once the bringup gates have opened.
    #[serde(default)]
    pub appcpu_released: bool,
```

- [ ] **Step 3: Update `capture_sim_state` to populate the new fields**

Find `capture_sim_state` in `crates/hw-oracle/src/lib.rs`. The function constructs a `Esp32s3Wiring` (or already has one in scope). After capturing PRO state, add:

```rust
    state.appcpu_released = wiring.core_controller.lock().unwrap().is_app_cpu_released();
    state.appcpu_pc = if state.appcpu_released {
        wiring.core_controller.lock().unwrap().entry().unwrap_or(0)
    } else {
        0
    };
```

(If the function captures into a different binding name, adjust accordingly.)

- [ ] **Step 4: Add the oracle test case**

Append to `crates/hw-oracle/tests/oracles.rs`:

```rust
#[test]
fn appcpu_bringup_captures_entry() {
    let case = OracleCase {
        name: "appcpu_bringup",
        elf: include_bytes!("../../../fixtures/xtensa-asm/appcpu_bringup.elf"),
        steps: 100_000,
    };
    let sim = run_sim(&case).expect("sim run");
    assert!(sim.appcpu_released, "controller flips to released");
    assert_eq!(
        sim.appcpu_pc, 0x4008_0123,
        "captured entry matches APP_ENTRY_LIT in fixture"
    );
}
```

(`OracleCase` and `run_sim` are existing helpers from Plan 4's oracle bank; adapt the call shape if the harness uses different field names.)

- [ ] **Step 5: Run the new oracle test**

Run: `cargo test -p labwired-hw-oracle --test oracles appcpu_bringup_captures_entry`
Expected: PASS.

- [ ] **Step 6: Run the full oracle suite to confirm no regressions**

Run: `cargo test -p labwired-hw-oracle`
Expected: all existing oracles still pass.

- [ ] **Step 7: Commit**

```bash
git add crates/hw-oracle/src/lib.rs crates/hw-oracle/tests/oracles.rs
git commit -m "test(hw-oracle): appcpu_bringup_captures_entry — Plan 5 dual-core oracle"
```

---

## Task 9: Firmware example — `examples/esp32s3-dual-core-tmp102/`

Mirror `examples/esp32s3-i2c-tmp102/` shape so the build/test machinery is identical.

**Files (all created):**
- `examples/esp32s3-dual-core-tmp102/Cargo.toml`
- `examples/esp32s3-dual-core-tmp102/.cargo/config.toml`
- `examples/esp32s3-dual-core-tmp102/rust-toolchain.toml`
- `examples/esp32s3-dual-core-tmp102/build.rs`
- `examples/esp32s3-dual-core-tmp102/src/main.rs`

- [ ] **Step 1: Copy the scaffolding from Plan 4**

Run:
```bash
cp -r examples/esp32s3-i2c-tmp102 examples/esp32s3-dual-core-tmp102
rm -f examples/esp32s3-dual-core-tmp102/README.md examples/esp32s3-dual-core-tmp102/RUNBOOK.md
rm -rf examples/esp32s3-dual-core-tmp102/target
```

- [ ] **Step 2: Update `Cargo.toml` package name + description**

Open `examples/esp32s3-dual-core-tmp102/Cargo.toml` and replace:

```toml
[package]
name = "esp32s3-dual-core-tmp102"
version = "0.1.0"
edition = "2021"
authors = ["LabWired Team <team@labwired.io>"]
license = "MIT"
description = "ESP32-S3 dual-core demo (Plan 5): PRO reads TMP102 + writes shared atomic; APP reads it + drives GPIO2."

[[bin]]
name = "esp32s3-dual-core-tmp102"
path = "src/main.rs"
```

(Keep the existing `[dependencies]` and `[profile.release]` blocks unchanged — esp-hal 1.1, esp-println 0.17, esp-backtrace 0.19, critical-section 1.2.)

- [ ] **Step 3: Replace `src/main.rs` with the dual-core firmware**

Open `examples/esp32s3-dual-core-tmp102/src/main.rs` and overwrite with:

```rust
//! ESP32-S3 dual-core demo for the LabWired simulator (Plan 5).
//!
//! PRO_CPU reads TMP102 once per second, writes the result (in 0.01 °C
//! units) into `SHARED_TEMP_CENTI_C`, and prints `[PRO] T = …`.
//! APP_CPU spins reading the shared atomic and drives GPIO2 high
//! whenever the temperature exceeds 30 °C, printing `[APP] LED=…` on
//! transitions only.

#![no_std]
#![no_main]

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use critical_section::Mutex;
use esp_backtrace as _;
use esp_hal::{
    gpio::{Level, Output, OutputConfig},
    handler,
    i2c::master::{Config as I2cConfig, I2c},
    main,
    system::{CpuControl, Stack},
    time::Duration,
    timer::{systimer::SystemTimer, PeriodicTimer},
};
use esp_println::println;

const TMP102_ADDR: u8 = 0x48;
const THRESHOLD_CENTI_C: i32 = 3000; // 30.00 °C

static SHARED_TEMP_CENTI_C: AtomicI32 = AtomicI32::new(i32::MIN);
static APP_RUN: AtomicBool = AtomicBool::new(true);

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

static mut APP_CPU_STACK: Stack<8192> = Stack::new();

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

    // ---- Spawn APP_CPU ---------------------------------------------------
    let mut cpu_ctrl = CpuControl::new(p.CPU_CTRL);
    let _guard = cpu_ctrl
        .start_app_core(
            unsafe { &mut *core::ptr::addr_of_mut!(APP_CPU_STACK) },
            move || app_main(led),
        )
        .expect("start APP_CPU");

    // ---- PRO_CPU loop ----------------------------------------------------
    loop {
        let tick = critical_section::with(|cs| {
            let v = *TICK_FLAG.borrow_ref(cs);
            TICK_FLAG.replace(cs, false);
            v
        });
        if tick {
            let mut buf = [0u8; 2];
            match i2c.write_read(TMP102_ADDR, &[0x00], &mut buf) {
                Ok(()) => {
                    let raw_u: u32 = ((buf[0] as u32) << 8) | (buf[1] as u32);
                    let units_16: i32 = (raw_u >> 4) as i32;
                    let centi_c: i32 = units_16 * 625 / 100;
                    SHARED_TEMP_CENTI_C.store(centi_c, Ordering::Release);
                    let int_part = centi_c / 100;
                    let frac_part = centi_c.unsigned_abs() % 100;
                    println!("[PRO] T = {}.{:02} C", int_part, frac_part);
                }
                Err(e) => println!("[PRO] I2C error: {:?}", e),
            }
        }
        core::hint::spin_loop();
    }
}

// ---- APP_CPU entry -------------------------------------------------------

fn app_main(mut led: Output<'static>) -> ! {
    let mut last_state = false;
    loop {
        if !APP_RUN.load(Ordering::Relaxed) {
            core::hint::spin_loop();
            continue;
        }
        let centi_c = SHARED_TEMP_CENTI_C.load(Ordering::Acquire);
        if centi_c == i32::MIN {
            // Producer hasn't published yet — keep spinning.
            core::hint::spin_loop();
            continue;
        }
        let on = centi_c > THRESHOLD_CENTI_C;
        if on != last_state {
            if on {
                led.set_high();
            } else {
                led.set_low();
            }
            println!("[APP] LED={}", if on { "high" } else { "low" });
            last_state = on;
        }
        core::hint::spin_loop();
    }
}
```

- [ ] **Step 4: Build the firmware**

Run:
```bash
cd examples/esp32s3-dual-core-tmp102 && cargo +esp build --release
```
Expected: clean build → `target/xtensa-esp32s3-none-elf/release/esp32s3-dual-core-tmp102` ELF. Return to repo root after.

If the `Stack`/`CpuControl` paths in esp-hal 1.1 differ, fix the import path with `cargo doc -p esp-hal --open` or `cargo expand`. The minimal viable signature is (from esp-hal 1.x):

```text
CpuControl::new(peripherals.CPU_CTRL)
   .start_app_core(&mut Stack<N>, FnOnce() -> !) -> Result<AppCoreGuard, _>
```

- [ ] **Step 5: Confirm the workspace cargo metadata picks it up**

Run: `cargo metadata --no-deps --format-version 1 | grep -o '"name":"esp32s3-dual-core-tmp102"'`
Expected: one match. If the workspace `Cargo.toml` doesn't include the new example as a member, add it under `[workspace] members`.

- [ ] **Step 6: Commit**

```bash
git add examples/esp32s3-dual-core-tmp102 Cargo.toml
git commit -m "feat(examples): esp32s3-dual-core-tmp102 firmware (Plan 5)"
```

---

## Task 10: Headline e2e test

**Files:**
- Create: `crates/core/tests/e2e_dual_core_tmp102.rs`

The test is structurally a clone of Plan 4's `e2e_i2c_tmp102.rs`, with three differences: (a) the test runs *both* cpu0 and cpu1 in the loop, (b) cpu1 starts only when `core_controller.is_app_cpu_released()`, (c) success requires `[PRO]` lines AND `[APP]` lines AND a GPIO2 transition.

- [ ] **Step 1: Create the test file**

Create `crates/core/tests/e2e_dual_core_tmp102.rs`:

```rust
// LabWired - Plan 5 Task 10: e2e test for dual-core firmware.

#![cfg(feature = "esp32s3-fixtures")]

use labwired_core::boot::esp32s3::{fast_boot, BootOpts};
use labwired_core::bus::SystemBus;
use labwired_core::peripherals::esp32s3::gpio::GpioObserver;
use labwired_core::peripherals::esp32s3::usb_serial_jtag::UsbSerialJtag;
use labwired_core::system::xtensa::{configure_xtensa_esp32s3, Esp32s3Opts};
use labwired_core::{Cpu, SimulationError};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
struct RecordingObserver {
    events: Mutex<Vec<(u8, bool, bool, u64)>>,
}
impl GpioObserver for RecordingObserver {
    fn on_pin_change(&self, pin: u8, from: bool, to: bool, sim_cycle: u64) {
        self.events.lock().unwrap().push((pin, from, to, sim_cycle));
    }
}

fn firmware_path() -> PathBuf {
    PathBuf::from(
        "../../examples/esp32s3-dual-core-tmp102/target/xtensa-esp32s3-none-elf/release/esp32s3-dual-core-tmp102",
    )
}

fn ensure_firmware_built() -> PathBuf {
    let elf = firmware_path();
    let src = PathBuf::from("../../examples/esp32s3-dual-core-tmp102/src/main.rs");
    if elf.exists() {
        if let (Ok(elf_meta), Ok(src_meta)) = (std::fs::metadata(&elf), std::fs::metadata(&src)) {
            if elf_meta.modified().unwrap() >= src_meta.modified().unwrap() {
                return elf;
            }
        }
    }
    let status = Command::new("cargo")
        .args(["+esp", "build", "--release"])
        .current_dir("../../examples/esp32s3-dual-core-tmp102")
        .status()
        .expect("cargo +esp build (toolchain installed and ~/export-esp.sh sourced?)");
    assert!(status.success(), "esp32s3-dual-core-tmp102 build failed");
    assert!(elf.exists(), "ELF not found at {elf:?} after build");
    elf
}

#[test]
fn dual_core_firmware_drives_led_via_shared_atomic() {
    let elf_path = ensure_firmware_built();
    let elf_bytes = std::fs::read(&elf_path).expect("read firmware ELF");

    let mut bus = SystemBus::new();
    let wiring = configure_xtensa_esp32s3(&mut bus, &Esp32s3Opts::default());

    let obs = Arc::new(RecordingObserver::default());
    wiring.add_gpio_observer(&mut bus, obs.clone());

    // Sink JTAG output for assertions.
    let jtag = Arc::new(Mutex::new(Vec::<u8>::new()));
    for p in bus.peripherals.iter_mut() {
        if let Some(any) = p.dev.as_any_mut() {
            if let Some(uj) = any.downcast_mut::<UsbSerialJtag>() {
                uj.set_sink(Some(jtag.clone()), false);
            }
        }
    }

    let icache_backing = wiring.icache_backing.clone();
    let dcache_backing = wiring.dcache_backing.clone();
    let core_controller = wiring.core_controller.clone();
    let mut cpu0 = wiring.cpu;
    let mut cpu1 = wiring.cpu1;

    fast_boot(
        &elf_bytes,
        &mut bus,
        &mut cpu0,
        &BootOpts {
            stack_top_fallback: 0x3FCD_FFF0,
            icache_backing: Some(icache_backing),
            dcache_backing: Some(dcache_backing),
        },
    )
    .expect("fast_boot");

    // Both cores get a clean reset state but share VECBASE etc.
    // PRO_CPU runs first; APP_CPU starts at the entry once gates open.
    let observers: Vec<Arc<dyn labwired_core::SimulationObserver>> = Vec::new();
    let cfg = labwired_core::SimulationConfig::default();

    const MAX_STEPS: u64 = 2_000_000_000;
    let mut app_started = false;

    for _ in 0..MAX_STEPS {
        // PRO_CPU step.
        match cpu0.step(&mut bus, &observers, &cfg) {
            Ok(()) => {}
            Err(SimulationError::BreakpointHit(_)) => break,
            Err(e) => panic!("PRO_CPU error at 0x{:08x}: {e}", cpu0.get_pc()),
        }

        // APP_CPU step (only after firmware releases it).
        let released = core_controller.lock().unwrap().is_app_cpu_released();
        if released {
            if !app_started {
                let entry = core_controller.lock().unwrap().entry().unwrap();
                cpu1.set_pc(entry);
                tracing::info!("APP_CPU released, pc=0x{entry:08x}");
                app_started = true;
            }
            match cpu1.step(&mut bus, &observers, &cfg) {
                Ok(()) => {}
                Err(SimulationError::BreakpointHit(_)) => break,
                Err(e) => panic!("APP_CPU error at 0x{:08x}: {e}", cpu1.get_pc()),
            }
        }

        let _ = bus.tick_peripherals_with_costs();

        // Early-out: ≥3 [PRO] lines AND ≥1 [APP] LED= line AND GPIO2 rose.
        let bytes = jtag.lock().unwrap();
        let text = String::from_utf8_lossy(&bytes).to_string();
        let pro_lines = text.lines().filter(|l| l.starts_with("[PRO] T = ")).count();
        let app_lines = text.lines().filter(|l| l.starts_with("[APP] LED=")).count();
        let gpio2_rose = obs
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|&(p, f, t, _)| p == 2 && !f && t);
        if pro_lines >= 3 && app_lines >= 1 && gpio2_rose {
            break;
        }
        if pro_lines >= 14 {
            // Failsafe: producer ran a long time without ever crossing
            // threshold or getting consumed. Bail out and let the assertion
            // produce a useful diagnostic.
            break;
        }
    }

    let bytes = jtag.lock().unwrap();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let pro_lines: Vec<&str> = text.lines().filter(|l| l.starts_with("[PRO] T = ")).collect();
    let app_lines: Vec<&str> = text.lines().filter(|l| l.starts_with("[APP] LED=")).collect();

    assert!(
        pro_lines.len() >= 3,
        "expected ≥3 [PRO] T = lines; got {}: {text:?}",
        pro_lines.len(),
    );
    assert!(
        !app_lines.is_empty(),
        "expected ≥1 [APP] LED= line; got 0; full text: {text:?}",
    );

    let gpio2_rose = obs
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|&(p, f, t, _)| p == 2 && !f && t);
    assert!(gpio2_rose, "GPIO2 must transition low → high (LED)");

    assert!(
        core_controller.lock().unwrap().is_app_cpu_released(),
        "controller must be released",
    );
}
```

If `XtensaLx7` exposes the program counter only through `get_pc()` and not a setter, add a `set_pc` method (or use the existing one — search `crates/core/src/cpu/xtensa_lx7.rs` for `fn set_pc` or `pub fn pc`).

- [ ] **Step 2: If `set_pc` doesn't exist on `XtensaLx7`, add it**

Run: `grep -n 'fn set_pc\|pub fn pc' crates/core/src/cpu/xtensa_lx7.rs | head -5`

If absent, add to `impl XtensaLx7`:

```rust
    /// Set the program counter directly. Used by the dual-core runner to
    /// jump APP_CPU to the entry address captured by `ets_set_appcpu_boot_addr`.
    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc;
    }
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p labwired-core --test e2e_dual_core_tmp102 --features esp32s3-fixtures -- --nocapture`
Expected: test PASSes; you should see interleaved `[PRO]` and `[APP]` output.

- [ ] **Step 4: Commit**

```bash
git add crates/core/tests/e2e_dual_core_tmp102.rs crates/core/src/cpu/xtensa_lx7.rs
git commit -m "test(plan-5): e2e dual-core demo — PRO publishes, APP consumes, GPIO2 toggles"
```

---

## Task 11: README + RUNBOOK

**Files:**
- Create: `examples/esp32s3-dual-core-tmp102/README.md`
- Create: `examples/esp32s3-dual-core-tmp102/RUNBOOK.md`

- [ ] **Step 1: Create README**

Write `examples/esp32s3-dual-core-tmp102/README.md`:

```markdown
# esp32s3-dual-core-tmp102

ESP32-S3 dual-core demo for the LabWired simulator (Plan 5).

PRO_CPU reads a TMP102 over I2C0 once per second, writes the temperature
(in 0.01 °C units) into a `static AtomicI32`, and prints it.
APP_CPU runs the consumer: it polls the atomic and toggles GPIO2 high
whenever temperature exceeds 30 °C, printing only on transitions.

## What this demo proves

* APP_CPU bringup via the real esp-hal `CpuControl::start_app_core` path
  (entry captured by `ets_set_appcpu_boot_addr`, gates released via
  `SYSTEM_CORE_1_CONTROL_0`).
* Per-core PRID: PRO_CPU reads 0xCDCD, APP_CPU reads 0xABAB.
* Shared-memory ordering using Xtensa LX7 `S32RI` (release-store) and
  `L32AI` (acquire-load) — these have been HW-oracle-validated since
  Plan 1.

## What it does NOT claim

* Cycle-accurate inter-core timing
* Cache modeling / region-aware load latency
* Inter-core software interrupts
* Embassy-on-dual-core

See `docs/superpowers/specs/2026-05-07-plan-5-dual-core-esp32s3-design.md`.

## Hardware wiring

* TMP102 SDA → GPIO8, SCL → GPIO9 (with 4.7 kΩ pull-ups to 3V3)
* LED → GPIO2

## Build

```bash
cd examples/esp32s3-dual-core-tmp102
cargo +esp build --release
```

## Run on the simulator

`cargo test -p labwired-core --test e2e_dual_core_tmp102 --features esp32s3-fixtures`

## Run on real silicon

See `RUNBOOK.md`.
```

- [ ] **Step 2: Create RUNBOOK**

Write `examples/esp32s3-dual-core-tmp102/RUNBOOK.md`:

```markdown
# RUNBOOK: esp32s3-dual-core-tmp102

## Reproducing on real ESP32-S3 silicon

### Prerequisites

* Rust ESP toolchain (`espup install`, then `source ~/export-esp.sh`)
* `espflash` (e.g. `cargo install espflash`)
* Any ESP32-S3 dev board (verified target: ESP32-S3-DevKitC-1)
* TMP102 breakout (Adafruit / SparkFun / equivalent)
* 4.7 kΩ pull-ups on SDA/SCL (most TMP102 breakouts omit these)

### Wiring

| Signal | Board pin | TMP102 / LED |
|--------|-----------|--------------|
| SDA    | GPIO8     | TMP102 SDA   |
| SCL    | GPIO9     | TMP102 SCL   |
| 3V3    | 3V3       | TMP102 VCC + 4k7 pull-ups |
| GND    | GND       | TMP102 GND + ADD0 (→ addr 0x48) |
| LED    | GPIO2     | LED + 330 Ω → GND |

### Build & flash

```bash
cd examples/esp32s3-dual-core-tmp102
cargo +esp build --release
espflash flash --monitor target/xtensa-esp32s3-none-elf/release/esp32s3-dual-core-tmp102
```

### Expected output

```
[PRO] T = 23.94 C
[PRO] T = 23.94 C
[APP] LED=low
[PRO] T = 31.06 C
[APP] LED=high
[PRO] T = 30.81 C
...
```

LED on GPIO2 follows the printed transitions. APP_CPU only logs on
state changes to keep the serial output legible.

### Cross-check with the simulator

```bash
cargo test -p labwired-core --test e2e_dual_core_tmp102 \
    --features esp32s3-fixtures -- --nocapture
```

The simulator's TMP102 model drifts the temperature monotonically, so
the timing of the LED transition will differ from real silicon — but
the sequence of events (PRO publishes → APP consumes → LED toggles) is
identical, and PRID 0xCDCD/0xABAB will match what's reported by
`esp-hal-extras::Cpu::current()` on either side.
```

- [ ] **Step 3: Commit**

```bash
git add examples/esp32s3-dual-core-tmp102/README.md examples/esp32s3-dual-core-tmp102/RUNBOOK.md
git commit -m "docs(esp32s3-dual-core-tmp102): README + RUNBOOK for Plan 5 demo"
```

---

## Task 12: Format, push, open PR

- [ ] **Step 1: Run formatters and lints**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: clean (no diff from fmt; no clippy warnings).

If clippy flags new warnings, fix them in-place and amend the relevant commit OR add a follow-up `chore(plan-5): clippy` commit.

- [ ] **Step 2: Run the full test matrix**

```bash
cargo test --workspace
cargo test -p labwired-core --test e2e_dual_core_tmp102 --features esp32s3-fixtures
cargo test -p labwired-core --test e2e_i2c_tmp102 --features esp32s3-fixtures   # regression
cargo test -p labwired-hw-oracle
```
Expected: all green.

- [ ] **Step 3: Push the branch**

```bash
git push -u origin plan-5-dual-core-esp32s3
```

- [ ] **Step 4: Open PR via `gh`**

```bash
gh pr create --title "feat(esp32s3): Plan 5 — dual-core bringup + producer/consumer demo" \
  --body "$(cat <<'EOF'
## Summary
- APP_CPU bringup gated by a shared `CoreController` populated by the
  `ets_set_appcpu_boot_addr` ROM thunk and `SYSTEM_CORE_1_CONTROL_0`
  writes (offset 0x18C, bits 0/1/2).
- Per-core `PRID` SR — PRO_CPU 0xCDCD, APP_CPU 0xABAB.
- Headline demo: `examples/esp32s3-dual-core-tmp102` — PRO reads TMP102
  + writes shared `AtomicI32`; APP reads it + drives GPIO2.
- Behavioral parity claim only (no cycle-accuracy claim, no inter-core
  software IRQs, no cache modeling).

Spec: `docs/superpowers/specs/2026-05-07-plan-5-dual-core-esp32s3-design.md`
Plan: `docs/superpowers/plans/2026-05-07-plan-5-dual-core-esp32s3.md`

## Test plan
- [ ] `cargo test --workspace` green
- [ ] `cargo test -p labwired-core --test xtensa_dual_core_bringup` green
- [ ] `cargo test -p labwired-core --test e2e_dual_core_tmp102 --features esp32s3-fixtures` green
- [ ] `cargo test -p labwired-hw-oracle` green
- [ ] Single-core regression: `cargo test -p labwired-core --test e2e_i2c_tmp102 --features esp32s3-fixtures` green
- [ ] CI green; merge via CI not admin
EOF
)"
```

- [ ] **Step 5: Wait for CI; merge only when green**

Per the project policy "no admin merges, just through CI", do not bypass CI. If CI fails, fix and push again. On green, squash-merge.

---

## Self-review notes (controller's audit)

**Spec coverage:** every spec section maps to a task — `CoreController` (Task 1), Bus accessor (Task 2), per-core PRID (Task 3), ROM thunk capture (Task 4), SystemStub gate routing (Task 5), Esp32s3Wiring threading (Task 6), bringup integration test (Task 7), oracle (Task 8), firmware (Task 9), e2e (Task 10), docs (Task 11), ship (Task 12). Out-of-scope items from the spec stay out — no tasks for `FROM_CPU_INTR_n`, embassy, cache modeling, or cycle accuracy.

**Type/name consistency:** `CoreController::set_entry/entry/set_clkgate_en/set_runstall/set_reset_en/is_app_cpu_released` is the same vocabulary used by SystemStub (Task 5), Esp32s3Wiring (Task 6), the bringup test (Task 7), and the e2e test (Task 10). `Arc<Mutex<CoreController>>` is the cross-component handle type throughout.

**No placeholders:** every step shows the actual code or command. The two "verify-and-add-if-missing" branches (Makefile generic rule in Task 7, `set_pc` in Task 10) include the exact code to add and the grep that drives the decision — they're not TBDs.
