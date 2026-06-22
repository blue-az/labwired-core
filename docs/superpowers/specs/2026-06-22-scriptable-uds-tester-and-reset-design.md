# Scriptable UDS tester + real Cortex-M system reset

- Status: approved (brainstorm), pending spec review
- Date: 2026-06-22
- Repo: labwired-core (branch `feat/scriptable-uds-tester`, PR → `main`)
- Motivation: udslib issue [#88](https://github.com/w1ne/udslib/issues/88) (0x11 ECUReset answers nothing when the reset reboots before the response is on the wire). udslib is fixed + unit-tested, but the ECU simulator cannot reproduce it end-to-end: the `uds-tester` is hardcoded to the SecurityAccess flow and the emulator does not honor a core reset.

## Goals

1. Make the simulator's `uds-tester` **scriptable** — drive any UDS service and assert its response.
2. Make the emulator perform a **real Cortex-M system reset** when firmware requests it (no shortcut model).
3. Reproduce #88 **end-to-end**: an ECU that actually reboots on `0x11`, with the harness asserting `51 01` reached the bus *before* the reboot.

Non-goals: rewriting ISO-TP, multi-tester buses, FD-only services. CAN-FD framing reuse only where the existing tester already supports it.

## Phase 1 — Scriptable UDS tester

`CanUdsTester` (`crates/core/src/bus/mod.rs`) today: fixed FSM `Start → AwaitFc → AwaitResp(0x67) → Done`, completion hardcoded at `mod.rs:278` (`data[0]==0x06 && data[1]==0x67`).

New config (`type: "uds-tester"`):

```yaml
config:
  request_id: 0x111
  reply_id: 0x222
  script:
    - send: "10 03"        # DiagnosticSessionControl extended
      expect: "50 03"
    - send: "11 01"        # ECUReset hardReset
      expect: "51 01"
    - send: "27 01"
      expect: "67 01 .."   # .. = one wildcard byte (variable seed)
```

Step grammar:
- `send: "<hex>"` — the raw UDS PDU (SID + payload). The harness frames it over ISO-TP: ≤7 bytes → single frame `0N ..`; >7 → FirstFrame, await FlowControl, ConsecutiveFrame(s).
- `expect: "<hex pattern>"` — match against the reassembled response PDU. `..` matches exactly one byte; a pattern shorter than the response is a prefix match.
- `expect_nrc: <byte>` (optional) — expect `7F <reqSID> <nrc>`.
- `expect_silence: true` (optional) — no reply within `timeout_ticks` is a pass (suppressPosRsp / functional broadcast).
- `timeout_ticks: <n>` (optional, default from `DEFAULT_MAX_TICKS`).

FSM generalized to iterate steps: per step `SendReq → AwaitResp → match → (next | Failed)`. Response reassembly handles SF and FF+CF (tester sends FlowControl). On the last step's match → `Done`. Any mismatch/timeout → `Failed { step, reason, expected, actual }`.

Backward compatibility: a config with legacy `first_frame`/`consecutive_frame` (no `script`) is translated internally into a single equivalent step, so the existing `f103-uds-ecu/uds-smoke.yaml` SecurityAccess scenario passes unchanged.

## Phase 1 — Assertion surfacing

Tester result is exposed to the scenario runner (`crates/cli/src/commands/test.rs`, alongside `uart_contains` / `expected_stop_reason`):

```yaml
assertions:
  - uds_tester: { id: "uds-tester", result: done }
```

On `Failed`, the runner prints the failing step index, expected pattern, and actual bytes, and fails the run. `result: done` requires every step matched.

## Phase 2 — Real Cortex-M system reset (no shortcut)

Today `SCB.AIRCR` writes are stored only (`scb.rs:150`); SYSRESETREQ is ignored. This must become a faithful reset:

- On a write to `AIRCR` with `VECTKEY == 0x05FA` (bits 31:16) and `SYSRESETREQ` (bit 2) set, request a system reset of the owning core.
- The reset must match silicon semantics, reusing the existing core `reset()` machinery (`lib.rs` / `world.rs`), not a bespoke path:
  - `MSP` reloaded from vector table `[0]`, `PC` from reset vector `[1]` (Thumb bit handling as on real hardware).
  - Core registers and the relevant system/peripheral state return to reset values per the chip descriptor (same path used at power-on, so RAM-vs-register reset behavior stays consistent with silicon-fidelity work already in the tree).
  - SRAM contents follow real-hardware behavior (system reset does not clear SRAM); only what silicon resets is reset.
- **Ordering for #88**: the response frame udslib hands to bxCAN/FDCAN must be emitted onto the bus before the core reset takes effect. The reset is serviced at a tick boundary after the CAN peripheral has drained the pending TX mailbox for that step, so a tester observes `51 01` then the reboot — never silence. This ordering is the property under test, so it must be real, not arranged by the test.

Validation that it's a *real* reset (not a flag): after SYSRESETREQ, firmware re-executes from the reset vector and re-runs its init (observable via UART banner reprinting, e.g. `ECU_READY` a second time).

## Phase 2 — Resetting ECU example + #88 smoke

- One ECU example (`f103-uds-ecu` or `h563-uds-ecu`) wires `fn_reset` → `NVIC_SystemReset` (writes AIRCR), built against udslib ≥ v1.20.0.
- Smoke scenario:

```yaml
script:
  - send: "11 01"
    expect: "51 01"
assertions:
  - uds_tester: { id: "uds-tester", result: done }   # 51 01 seen before reboot
  - uart_contains: "ECU_READY"                        # banner reprinted => real reboot
```

A pre-fix udslib (≤ v1.14.0) at the example's `UDSLIB_DIR` would leave the tester `Failed` (no `51 01`), giving an end-to-end regression for #88.

## Testing

- Rust unit tests in `bus/mod.rs`: script parsing, SF framing, FF/FC/CF framing, wildcard match, `expect_nrc`, `expect_silence`, legacy-config translation.
- Rust unit test in `scb.rs`/core: AIRCR SYSRESETREQ triggers a core reset with correct MSP/PC reload; non-key writes do not.
- Scenario tests: existing SecurityAccess smoke still green; new `0x11`-reset smoke green on udslib ≥ v1.20.0.
- Gate: core-integrity must pass; PR targets `main`.

## Risks

- Reset/CAN TX ordering at the tick boundary is the crux — must verify the frame is on the bus before reset, deterministically.
- Touching the CPU/SCB reset path risks regressions in unrelated chips; keep the trigger SCB-local and route through the existing reset machinery, covered by tests.
