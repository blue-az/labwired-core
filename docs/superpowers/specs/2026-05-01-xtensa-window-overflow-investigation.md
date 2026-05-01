# Xtensa Window Overflow Exception — investigation findings

**Date:** 2026-05-01
**Branch:** `fix/xtensa-window-exception-model` (canonical model attempt, NOT merged)
**Status:** Investigation complete; canonical model implemented but cannot be empirically validated against ESP32-S3 silicon with current tooling.

## Background

Plan 1's Xtensa LX7 model implements ENTRY's overflow path as:

- WB unchanged on entry
- PS.OWB unchanged
- RFWO advances WB by CALLINC

This works for the H7 oracle dispatch tests (which only sample WB immediately at the vector address — see "Why H7 oracle tests didn't catch this" below) and for shallow ESP32-S3 firmware (`e2e_blinky`, `e2e_hello_world`). It breaks for any firmware that uses the standard `xtensa-lx-rt` overflow vectors:

```asm
_WindowOverflow4:
    s32e a0, a5, -16
    s32e a1, a5, -12
    s32e a2, a5,  -8
    s32e a3, a5,  -4
    rfwo
```

The Plan 4 e2e firmware (`esp32s3-i2c-tmp102`) trips this at the first OF8 dispatch — handler's `a1` is junk (`0x1`), so `l32e a0, a1, -12` faults at `0xfffffff5`.

## Canonical reference (QEMU)

QEMU's `target/xtensa/win_helper.c::HELPER(window_check)` documents the canonical Xtensa overflow exception entry:

```c
uint32_t windowstart = xtensa_replicate_windowstart(env) >> (env->sregs[WINDOW_BASE] + 1);
uint32_t n = ctz32(windowstart) + 1;
xtensa_rotate_window(env, n);
env->sregs[PS] = (env->sregs[PS] & ~PS_OWB) | (windowbase << PS_OWB_SHIFT) | PS_EXCM;
env->sregs[EPC1] = env->pc = pc;
// Dispatch to OF4/OF8/OF12 based on subsequent WS bit gaps.
```

And `translate_rfw` makes RFWO/RFWU simple:

```c
PS &= ~PS_EXCM;
WindowStart[WB] = 0 (RFWO) or 1 (RFWU);
WB = PS.OWB;
PC = EPC1;
```

I implemented this canonical model on branch `fix/xtensa-window-exception-model`, commit `754f1e4`. All 524 sim tests passed (assertions for OF4/OF8/OF12/UF4/UF8/UF12 + RFWO/RFWU updated to expect rotated WindowBase and saved OWB). H7 oracle assertions also updated.

## What we observed on ESP32-S3 silicon

Tested with hw-oracle harness against the user's connected ESP32-S3-Zero board. Key probe (`crates/hw-oracle/tests/hw_probe.rs::hw_probe_overflow_captures_wb_on_silicon`):

Setup (matching H7.2):
- WB = 0
- WS = 0x0005 (bits 0, 2)
- VECBASE = IRAM_BASE+0x800
- Program: CALL4 at IRAM_BASE+0 → ENTRY at IRAM_BASE+8
- OF4 vector content: `j .` (infinite loop, intended to halt cleanly via OpenOCD force-halt, avoiding BREAK→DoubleException)

Observed at halt:
- **PC = 0x40370BC0** = vec+0x3C0 = DoubleExceptionVector
- **WB = 0** (unrotated)
- **PS.OWB = 0**
- **PS.CALLINC = 1** (preserved from CALL4 — confirms ENTRY did fire overflow)
- PS.EXCM = 1
- a4 = 0x40370003 (CALL4 return address — confirms CALL4 ran)

The `j .` did not loop — silicon executed something at vec+0 that triggered another exception (with EXCM=1 → DoubleException). Most likely cause: ESP32-S3's I-cache is not invalidated by OpenOCD's `mww`, so the CPU fetched stale data at vec+0.

**Conclusion:** the probe halts at DoubleException entry, not at OF4 entry. WB=0 at that point is consistent with EITHER:
- Silicon doesn't rotate on OF entry (Plan 1 model is correct)
- Silicon does rotate, but DoubleException entry restores WB

We cannot disambiguate without a probe that reaches the OF4 vector first instruction without triggering further exceptions.

## Why the H7 oracle tests didn't catch this

H7.2 (`entry_window_overflow_of4`) places a BREAK at the OF4 vector. With PS.EXCM=1 (set by overflow entry), BREAK fires DoubleException → PC = vec+0x3C0. The H7.2 assertion `st.assert_pc(IRAM_BASE + 0x800)` sees PC at the OF4 vector slot, but only because OpenOCD's `wait_until_halted` happens to read the PC at a moment that includes the OF4-entry PC in the halt context (DEPC etc).

Specifically: the PC reported is the BREAK's PC (vec+0), not the post-DoubleException PC (vec+0x3C0). But that's just how OpenOCD reports "the BREAK that caused this halt" via the debug exception PC.

So H7.2 validates the OF4 vector dispatch but **does NOT validate WindowBase rotation behavior**. The `st.assert_windowbase(0)` is consistent with both no-rotation (silicon model) and rotation-then-restore. The assertion is technically correct for the captured state but uninformative about what happens at the OF4 vector entry instruction.

## Plan 4 firmware OF8 trace

When the actual `esp32s3-i2c-tmp102` firmware triggers OF8:

```
[OF8] pc=0x4201afc4 wb=13 ws=0x2aab cinc=2 n=3 wb_h=0
  a1@WB+1 = 0x1   (= a1 of frame at WB=0 after rotation by n=3)
  a9@WB+1 = 0x18
```

With QEMU's algorithm, n=3, wb_handler=0. Frame 0 should be a "live" frame (WS[0]=1) with valid SP. But its a1 is `0x1` (junk).

This suggests one of:
1. ESP32-S3 silicon doesn't actually rotate by `n=3` (matches our observation)
2. Frame 0's a1 was overwritten somewhere in the boot/call chain we don't model
3. Some implicit S32E side-effects on PS.OWB or AR-file state

## What's blocking definitive answer

The H7.2 silicon probe is contaminated by BREAK → DoubleException. To definitively answer "does WB rotate on OF entry?", a probe needs to halt INSIDE the handler **before** any windowed instruction that would trigger DoubleException-via-implicit-exception.

Approaches (untried):
- Manual I-cache invalidation via OpenOCD before resume
- Use `WAITI 15` instead of `j .` (puts CPU in low-power wait, easier to force-halt)
- Use a non-windowed instruction sequence in the OF vector that just sets EXCSAVE1 = WB then `WSR.PS` to clear EXCM then `BREAK`
- Use OpenOCD's `xtensa step` to single-step from OF entry

Each requires more probe iteration than was practical in this session.

## Decision

**Keep the SEXT decoder fix** — it's an independent, definitively correct fix for a real `IllegalInstruction` fault in esp-hal firmware. (Plan 1 originally only decoded SEXT under `op0=3, op1=0`; esp-hal compiles SEXT as `op0=0, op1=3, op2=2`, which Plan 1 trapped.)

**Do NOT merge the canonical window model.** It might be wrong for ESP32-S3. Without HW validation, merging would risk regressing the simpler firmware tests that currently pass with Plan 1's model.

**Plan 4 e2e remains blocked** until either:
- The window model question is conclusively resolved with HW evidence
- OR a custom OF/UF vector is written into the test firmware that bypasses the standard handler's assumptions

## Branches and PRs

- `fix/xtensa-window-exception-model` (commit `754f1e4`, pushed) — canonical model implementation, kept for reference, NOT merged.
- `fix/xtensa-sext-decoder-and-window-investigation` (this branch) — SEXT decoder fix + this doc, ready for review.

## References

- QEMU xtensa target: <https://github.com/qemu/qemu/tree/master/target/xtensa>
- xtensa-lx-rt window vectors: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/xtensa-lx-rt-0.22.0/src/exception/asm.rs`
- Cadence Xtensa LX ISA Reference Manual §4.7.1 (Window Overflow Exceptions)
- ESP32-S3 TRM v1.4 §3 (Memory Map and Caches)
