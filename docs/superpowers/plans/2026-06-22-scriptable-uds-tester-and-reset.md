# Scriptable UDS Tester + Real Cortex-M System Reset Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the ECU simulator drive any UDS service from a YAML script and assert its response, and make the emulator perform a real Cortex-M system reset so udslib issue #88 (0x11 answers before reboot) can be reproduced end-to-end.

**Architecture:** Generalize `CanUdsTester` (`crates/core/src/bus/mod.rs`) from a hardcoded SecurityAccess FSM into a script-stepped request/expect engine that auto-frames ISO-TP. Add a `uds_tester` scenario assertion in the CLI runner. Make `SCB.AIRCR` honor SYSRESETREQ by latching a request that `Machine::step` drains and routes through the existing `Machine::reset()` — mirroring the established `drain_rtc_cntl_reset_request` pattern.

**Tech Stack:** Rust (`labwired-core`, `labwired-cli`), serde_yaml, cargo test; ARM bare-metal example firmware (arm-none-eabi-gcc).

## Global Constraints

- Repo labwired-core has **no `develop`** — branch from latest `origin/main`; PR targets `main`. (Working branch: `feat/scriptable-uds-tester`.)
- Integrate with `git merge`, never rebase. No "Claude"/AI references in commits. Commit identity: `w1ne <14119286+w1ne@users.noreply.github.com>`.
- Must pass the **core-integrity** gate (clippy is default-members, not `--workspace`).
- Reset must be a **real** silicon-faithful Cortex-M reset reusing existing `Machine::reset()` machinery — no shortcut/flag model. SRAM is not cleared by system reset; only what silicon resets is reset.
- Backward compatibility: the existing `examples/f103-uds-ecu/uds-smoke.yaml` SecurityAccess scenario must still pass unchanged.
- Core crate: `labwired-core`. CLI crate: `labwired-cli`, binary `labwired`. Scenario runner entry: `crates/cli/src/commands/test.rs::run_test`.

---

## Phase 1 — Scriptable UDS tester

### Task 1: Script data model + config parsing

**Files:**
- Modify: `crates/core/src/bus/mod.rs` (the `CanUdsTester` struct ~209-286 and its config construction)
- Test: `crates/core/src/bus/mod.rs` (`#[cfg(test)]` module, alongside `uds_tester_parsed_from_config` ~2110)

**Interfaces:**
- Produces:
  - `pub struct UdsStep { pub send: Vec<u8>, pub expect: Vec<Option<u8>>, pub expect_nrc: Option<u8>, pub expect_silence: bool, pub timeout_ticks: u64 }` (`expect` byte `None` = `..` wildcard).
  - `CanUdsTester` gains `pub script: Vec<UdsStep>`, `pub step_idx: usize`, and `pub failure: Option<String>`.
  - `fn parse_script(value: Option<&serde_yaml::Value>) -> Vec<UdsStep>` and `fn parse_expect(s: &str) -> Vec<Option<u8>>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn uds_script_parses_send_expect_and_wildcards() {
    let manifest: SystemManifest = serde_yaml::from_str(
        r#"
name: "uds-script"
chip: "f103"
external_devices:
  - id: "uds-tester"
    type: "uds-tester"
    connection: "bxcan1"
    config:
      request_id: "0x111"
      reply_id: "0x222"
      script:
        - send: "11 01"
          expect: "51 01"
        - send: "27 01"
          expect: "67 01 .."
board_io: []
"#,
    )
    .unwrap();
    let chip: ChipDescriptor = serde_yaml::from_str(MIN_F103_CHIP).unwrap();
    let bus = SystemBus::from_config(&chip, &manifest).unwrap();
    let t = &bus.can_uds_testers[0];
    assert_eq!(t.script.len(), 2);
    assert_eq!(t.script[0].send, vec![0x11, 0x01]);
    assert_eq!(t.script[0].expect, vec![Some(0x51), Some(0x01)]);
    assert_eq!(t.script[1].expect, vec![Some(0x67), Some(0x01), None]); // .. = wildcard
}
```

(Define `const MIN_F103_CHIP: &str` once at the top of the test module reusing the chip yaml already inlined in `uds_tester_parsed_from_config`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p labwired-core uds_script_parses_send_expect_and_wildcards`
Expected: FAIL — `script` field / `UdsStep` does not exist.

- [ ] **Step 3: Write minimal implementation**

Add the struct and parsing. `parse_expect` splits on whitespace; `".."` → `None`, else `u8::from_str_radix(tok.trim_start_matches("0x"), 16)` → `Some`. `parse_script` walks the `script` sequence reading `send` (reuse existing `yaml_bytes`), `expect` (via `parse_expect`), optional `expect_nrc` (`yaml_u32 as u8`), `expect_silence` (bool), `timeout_ticks` (`yaml_u32 as u64`, default `DEFAULT_MAX_TICKS`). In the uds-tester construction branch, populate `script`; initialize `step_idx: 0`, `failure: None`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p labwired-core uds_script_parses_send_expect_and_wildcards`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/bus/mod.rs
git commit -m "feat(sim): UDS tester script model + config parsing"
```

---

### Task 2: Legacy-config translation (keep SecurityAccess smoke green)

**Files:**
- Modify: `crates/core/src/bus/mod.rs` (uds-tester construction)
- Test: `crates/core/src/bus/mod.rs` test module

**Interfaces:**
- Consumes: `UdsStep`, `CanUdsTester.script` (Task 1).
- Produces: when no `script:` key is present, a one-step script synthesized from `first_frame`/`consecutive_frame` with `expect` = `[Some(0x06), Some(0x67)]` (the legacy SecurityAccess completion).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn uds_legacy_config_becomes_one_step_script() {
    let manifest: SystemManifest = serde_yaml::from_str(
        r#"
name: "uds-legacy"
chip: "f103"
external_devices:
  - id: "uds_node"
    type: "uds-tester"
    connection: "bxcan1"
    config:
      request_id: "0x111"
      reply_id: "0x222"
      first_frame: "10 0B 27 01 5A 11 22 33"
      consecutive_frame: "21 44 55 66 77 88 55 55"
board_io: []
"#,
    )
    .unwrap();
    let chip: ChipDescriptor = serde_yaml::from_str(MIN_F103_CHIP).unwrap();
    let bus = SystemBus::from_config(&chip, &manifest).unwrap();
    let t = &bus.can_uds_testers[0];
    assert_eq!(t.script.len(), 1);
    assert_eq!(t.script[0].expect, vec![Some(0x06), Some(0x67)]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p labwired-core uds_legacy_config_becomes_one_step_script`
Expected: FAIL — legacy path produces empty `script`.

- [ ] **Step 3: Write minimal implementation**

In the construction branch: if the `script` key is absent, build `vec![UdsStep { send: <decoded from first/consecutive frames as the raw UDS PDU 27 01 5A 11 22 33 44 55 66 77 88>, expect: vec![Some(0x06), Some(0x67)], expect_nrc: None, expect_silence: false, timeout_ticks: DEFAULT_MAX_TICKS }]`. Keep `first_frame`/`consecutive_frame` fields for the raw-injection path used by the framing engine in Task 3.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p labwired-core uds_legacy_config_becomes_one_step_script`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/bus/mod.rs
git commit -m "feat(sim): translate legacy uds-tester config to a one-step script"
```

---

### Task 3: ISO-TP framing + response reassembly + per-step FSM

**Files:**
- Modify: `crates/core/src/bus/mod.rs` — `CanUdsTesterState`, `observe_ecu_frame` (~265), `service_can_uds_testers` (~610)
- Test: `crates/core/src/bus/mod.rs` test module

**Interfaces:**
- Consumes: `UdsStep`, `script`, `step_idx`, `failure`.
- Produces: FSM that, per step, transmits the request (SF if `send.len() <= 7`, else FF + await FC + CF), reassembles the reply (SF or FF+CF with the tester emitting its own FlowControl), matches against `expect`/`expect_nrc`/`expect_silence`, then advances `step_idx` or sets `state = Failed` + `failure`. All steps matched → `Done`. New internal helpers `fn build_request_frames(&self) -> Vec<Vec<u8>>` and `fn matches(resp: &[u8], step: &UdsStep) -> bool`.

- [ ] **Step 1: Write the failing test (single-frame request, exact reply)**

```rust
#[test]
fn uds_tester_single_step_sf_request_matches_reply() {
    // Build a bus with the f103 + a 1-step script: send 11 01, expect 51 01.
    let mut bus = bus_with_script(&[("11 01", "51 01")]);
    // ECU emits the SF positive response 02 51 01 on reply_id.
    bus.inject_ecu_reply(0x222, &[0x02, 0x51, 0x01]);
    bus.service_can_uds_testers();
    assert_eq!(bus.can_uds_testers[0].state, CanUdsTesterState::Done);
}
```

(Add two small test-only helpers in the test module: `bus_with_script(steps)` constructs the manifest+bus; `inject_ecu_reply(id, bytes)` pushes a `CanFrame` into the connected bxCAN `tx_frames` so the next `service_can_uds_testers` drains it. Model them on `uds_tester_fsm_drives_ff_fc_cf_response` ~1984.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p labwired-core uds_tester_single_step_sf_request_matches_reply`
Expected: FAIL — current FSM only completes on `06 67`.

- [ ] **Step 3: Write minimal implementation**

Replace the hardcoded completion. In `AwaitResp`, reassemble: SF (`data[0] & 0xF0 == 0x00`, len `data[0] & 0x0F`) yields the PDU directly; FF (`0x10`) starts reassembly, tester replies FlowControl `30 00 00`, CFs (`0x2N`) append until the FF-declared length is reached. On a complete PDU call `matches`; on match advance `step_idx` (→ build next request, or `Done` if none left); on mismatch set `Failed` + `failure`. `matches` checks `expect_nrc` (`7F <send[0]> <nrc>`), `expect_silence` (handled in the timeout branch: silence to `timeout_ticks` = pass), else compares `expect` element-wise treating `None` as wildcard, allowing the response to be longer than the pattern (prefix match). `build_request_frames` produces SF or FF+CF list for the current step's `send`. Drive sends in `service_can_uds_testers` from `script[step_idx]` instead of `first_frame`/`consecutive_frame`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p labwired-core uds_tester_single_step_sf_request_matches_reply`
Expected: PASS

- [ ] **Step 5: Add multiframe + wildcard + nrc tests**

```rust
#[test]
fn uds_tester_wildcard_and_multistep() {
    let mut bus = bus_with_script(&[("10 03", "50 03"), ("27 01", "67 01 ..")]);
    bus.inject_ecu_reply(0x222, &[0x02, 0x50, 0x03]); bus.service_can_uds_testers();
    assert_eq!(bus.can_uds_testers[0].step_idx, 1);
    bus.inject_ecu_reply(0x222, &[0x03, 0x67, 0x01, 0xAB]); bus.service_can_uds_testers();
    assert_eq!(bus.can_uds_testers[0].state, CanUdsTesterState::Done);
}

#[test]
fn uds_tester_nrc_mismatch_fails_with_reason() {
    let mut bus = bus_with_script(&[("11 01", "51 01")]);
    bus.inject_ecu_reply(0x222, &[0x03, 0x7F, 0x11, 0x22]); // NRC, not 51 01
    bus.service_can_uds_testers();
    assert_eq!(bus.can_uds_testers[0].state, CanUdsTesterState::Failed);
    assert!(bus.can_uds_testers[0].failure.as_ref().unwrap().contains("step 0"));
}
```

- [ ] **Step 6: Run all tester tests**

Run: `cargo test -p labwired-core uds_tester`
Expected: PASS (incl. legacy `uds_tester_fsm_drives_ff_fc_cf_response`)

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/bus/mod.rs
git commit -m "feat(sim): script-stepped UDS tester FSM with ISO-TP framing + matching"
```

---

### Task 4: `uds_tester` scenario assertion in the CLI runner

**Files:**
- Modify: `crates/cli/src/commands/test.rs` (assertion enum/parse + evaluation, alongside `uart_contains`/`expected_stop_reason`)
- Test: `crates/cli/tests/outputs.rs` (or the existing scenario-assertion test file; follow the pattern there)

**Interfaces:**
- Consumes: post-run access to `bus.can_uds_testers` (state + `failure`). If the runner does not already expose the bus after a run, add a read-only getter on the run result.
- Produces: assertion `- uds_tester: { id: "<id>", result: done }`. `done` requires the named tester `state == Done`; otherwise the assertion fails and prints `tester <id>: <failure or current state>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn uds_tester_assertion_passes_when_done() {
    // Drive a minimal scenario whose ECU answers 51 01; assert result: done.
    let out = run_scenario_str(SCN_RESET_OK); // helper in this test file
    assert!(out.passed, "expected pass, got: {}", out.report);
}
```

(Define `SCN_RESET_OK` inline: a tiny scenario using an existing ELF that answers `0x11`, or a stub bus. If no ELF is convenient at unit scope, place this as an integration test under `crates/cli/tests/` driving a fixture scenario.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p labwired-cli uds_tester_assertion_passes_when_done`
Expected: FAIL — `uds_tester` assertion type unknown.

- [ ] **Step 3: Write minimal implementation**

Add the `uds_tester { id, result }` variant to the assertion parser and evaluation. After the run completes, look up the tester by `id` in `bus.can_uds_testers`; pass iff `state == Done` for `result: done`. On failure include `failure` text.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p labwired-cli uds_tester_assertion_passes_when_done`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/commands/test.rs crates/cli/tests/
git commit -m "feat(cli): uds_tester scenario assertion (result: done)"
```

---

## Phase 2 — Real Cortex-M system reset

### Task 5: SCB AIRCR SYSRESETREQ latch

**Files:**
- Modify: `crates/core/src/peripherals/scb.rs` (struct + `write_register` arm `0x0C`)
- Test: `crates/core/src/peripherals/scb.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `Scb` gains `pending_reset: std::cell::Cell<bool>` (init false); `pub fn drain_reset_request(&self) -> bool` (returns and clears). A write to AIRCR with `(value >> 16) == 0x05FA` and `value & (1 << 2) != 0` sets the latch; `aircr` still stores the value masked of the write key (read-back returns 0 in VECTKEY, as on silicon).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn aircr_sysresetreq_with_vectkey_latches_reset() {
    let scb = Scb::new();
    scb.write_register(0x0C, (0x05FA << 16) | (1 << 2)); // VECTKEY + SYSRESETREQ
    assert!(scb.drain_reset_request());
    assert!(!scb.drain_reset_request()); // latch cleared
}

#[test]
fn aircr_without_vectkey_does_not_reset() {
    let scb = Scb::new();
    scb.write_register(0x0C, 1 << 2); // SYSRESETREQ but no key
    assert!(!scb.drain_reset_request());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p labwired-core aircr_sysresetreq`
Expected: FAIL — `drain_reset_request` missing.

- [ ] **Step 3: Write minimal implementation**

Add `pending_reset: Cell<bool>` to `Scb` and `Default`/`new`. In `write_register` `0x0C`: `if (value >> 16) == 0x05FA && value & (1 << 2) != 0 { self.pending_reset.set(true); }` then `self.aircr = value & 0x0000_FFFF;`. Add `pub fn drain_reset_request(&self) -> bool { self.pending_reset.replace(false) }`. (If `write_register` takes `&self`, `Cell` fits; if `&mut self`, use a plain `bool` and `std::mem::take`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p labwired-core aircr_sysresetreq aircr_without_vectkey`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/peripherals/scb.rs
git commit -m "feat(sim): SCB AIRCR SYSRESETREQ latch (drain_reset_request)"
```

---

### Task 6: Route SCB reset through Machine::reset at an instruction boundary

**Files:**
- Modify: `crates/core/src/lib.rs` — add `scb_index` (resolve in constructor like `rtc_cntl_index`), `fn drain_scb_reset_request(&self) -> bool`, and a call site in `step()` next to `drain_rtc_cntl_reset_request`
- Test: `crates/core/src/lib.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `Scb::drain_reset_request` (Task 5), existing `Machine::reset()` (`lib.rs:865`).
- Produces: after the instruction that writes AIRCR completes, `step()` calls `self.reset()`, reloading MSP from vector[0] and PC from the reset vector via the existing CPU reset path. No new reset semantics.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn sysresetreq_reboots_cpu_via_vector_table() {
    // Minimal Cortex-M machine: vector table at flash base => [0]=MSP, [1]=reset+1.
    let mut m = test_machine_m3_with_vectors(0x2000_1000, 0x0800_0101);
    // Firmware writes AIRCR = 05FA0004 then would spin; we just step once past it.
    m.poke_u32(SCB_AIRCR, (0x05FA << 16) | (1 << 2));
    m.scb_set_pending_for_test(); // or execute the store; helper sets the latch
    m.step().unwrap();
    assert_eq!(m.cpu.pc() & !1, 0x0800_0100);
    assert_eq!(m.cpu.sp(), 0x2000_1000);
}
```

(Use the existing Cortex-M test-machine constructor in this module; if none exposes vector seeding, add a small `#[cfg(test)]` helper mirroring how power-on reset is tested.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p labwired-core sysresetreq_reboots_cpu_via_vector_table`
Expected: FAIL — `step()` does not honor SCB reset.

- [ ] **Step 3: Write minimal implementation**

Mirror `drain_rtc_cntl_reset_request`: add `scb_index: Option<usize>` resolved in the constructor (find peripheral downcasting to `Scb`). Add:

```rust
fn drain_scb_reset_request(&self) -> bool {
    let Some(idx) = self.scb_index else { return false; };
    let Some(p) = self.bus.peripherals.get(idx) else { return false; };
    p.dev.as_any()
        .and_then(|a| a.downcast_ref::<crate::peripherals::scb::Scb>())
        .map(|scb| scb.drain_reset_request())
        .unwrap_or(false)
}
```

In `step()`, next to the RTC_CNTL drain: `if self.drain_scb_reset_request() { self.reset()?; }`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p labwired-core sysresetreq_reboots_cpu_via_vector_table`
Expected: PASS

- [ ] **Step 5: Run the full core suite (no regressions)**

Run: `cargo test -p labwired-core`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/lib.rs
git commit -m "feat(sim): honor SCB SYSRESETREQ via Machine::reset at instruction boundary"
```

---

### Task 7: Resetting ECU example + #88 end-to-end smoke

**Files:**
- Modify: `examples/f103-uds-ecu/firmware/main.c` (wire `fn_reset` → `NVIC_SystemReset`; print a banner on boot so the reprint proves a real reboot)
- Create: `examples/f103-uds-ecu/uds-reset-smoke.yaml`
- Create/Modify: `examples/f103-uds-ecu/system.yaml` may be reused; if the smoke needs the script tester, add a sibling system file or extend with the `script:` config
- Test: scenario run via `labwired test`

**Interfaces:**
- Consumes: scriptable tester (Tasks 1-4), real reset (Tasks 5-6).
- Produces: a green scenario that sends `11 01`, asserts `51 01` (tester `done`), and asserts the boot banner appears twice (real reboot). Built against udslib ≥ v1.20.0.

- [ ] **Step 1: Add the reset callback + boot banner to the firmware**

In `main.c`: add `static void ecu_reset(uds_ctx_t *ctx, uint8_t type) { (void)ctx; (void)type; NVIC_SystemReset(); }`, set `cfg.fn_reset = ecu_reset;`, and ensure the init path prints `ECU_READY` on every boot (so a reboot reprints it). Confirm `UDSLIB_DIR` resolves to a v1.20.0+ source tree.

- [ ] **Step 2: Write the smoke scenario**

```yaml
# examples/f103-uds-ecu/uds-reset-smoke.yaml
schema_version: "1.0"
inputs:
  system: "./system.yaml"        # extend external_devices with the script below
  firmware: "./firmware/build/f103_uds_ecu.elf"
limits:
  max_steps: 600000
assertions:
  - uart_contains: "ECU_READY"
  - uds_tester: { id: "uds-tester", result: done }   # 51 01 received before reboot
```

In `system.yaml` (or a smoke-specific copy) set the tester config:

```yaml
    config:
      request_id: 0x111
      reply_id: 0x222
      script:
        - send: "11 01"
          expect: "51 01"
```

- [ ] **Step 3: Build the firmware**

Run: `make -C examples/f103-uds-ecu/firmware UDSLIB_DIR=<path to udslib v1.20.0+>`
Expected: `f103_uds_ecu.elf` produced.

- [ ] **Step 4: Run the smoke and verify it passes**

Run: `cargo run -p labwired-cli --bin labwired -- test examples/f103-uds-ecu/uds-reset-smoke.yaml`
Expected: PASS — tester `done` (saw `51 01`) and `ECU_READY` present.

- [ ] **Step 5: Confirm it reproduces #88 against pre-fix udslib (manual, optional but recommended)**

Rebuild the firmware with `UDSLIB_DIR` pointed at udslib v1.14.0 and rerun the smoke.
Expected: FAIL — tester `Failed` (no `51 01`), proving the end-to-end regression catches #88.

- [ ] **Step 6: Commit**

```bash
git add examples/f103-uds-ecu/
git commit -m "test(sim): end-to-end #88 reset smoke — 0x11 answers 51 01 before reboot"
```

---

## Final: gate + PR

- [ ] Run the gate locally: `cargo test -p labwired-core && cargo test -p labwired-cli && cargo clippy` (default-members).
- [ ] Push branch and open PR → `main`. PR body references udslib #88 and links the spec. Confirm core-integrity is green before merge.

---

## Self-Review

- **Spec coverage:** scriptable tester (Tasks 1-3), assertion surfacing (Task 4), real Cortex-M reset reusing `Machine::reset` (Tasks 5-6), resetting ECU + end-to-end #88 smoke (Task 7), backward-compat legacy translation (Task 2). All spec sections mapped.
- **Placeholder scan:** no TBD/TODO; each code step shows code; test helpers (`bus_with_script`, `inject_ecu_reply`, `test_machine_m3_with_vectors`) are named with their construction basis cited.
- **Type consistency:** `UdsStep` fields, `script`/`step_idx`/`failure`, `drain_reset_request`, `drain_scb_reset_request` used consistently across tasks; assertion shape `uds_tester { id, result }` consistent between Task 4 and Task 7.
