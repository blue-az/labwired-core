# Pending Silicon Verification

Standing rule: **every chip-model fix is provisional until verified against real
hardware.** A sim-consistent green in the Tier-1 matrix proves the model is
internally consistent with the fixture; only silicon breaks the circularity.
Each model-behavior change adds an entry here in the same PR; an entry closes
when its hardware verification lands (and the matrix cell graduates to the
silicon-anchored tier with the HIL workstream).

| # | Model change | PR/commit | HW verification recipe | Board | Status |
|---|---|---|---|---|---|
| 1 | Bit-band translation gated on core (M3/M4 only); H5/WBA GPIO un-shadowed | `ee1133c` | MMIO capture of GPIO word-writes at 0x4202_xxxx on silicon, replayed via the hw-oracle diff harness (pattern: `l476_mmio_diff`) | NUCLEO-H563ZI | open |
| 2 | T1 shift-immediate flags suppressed inside IT blocks | `60445bd` | Instruction-level oracle: IT-block sequences with T1 LSL/LSR/ASR, APSR captured on silicon (extend `thumb_oracles`) | any Cortex-M3/M4 board (F103 capture scripts exist) | open |
| 3 | Thumb-1 STRH/LDRSB/LDRH/LDRSH register-offset decode | `4ebed86` | Same `thumb_oracles` extension: loaded/stored values + sign-extension vs silicon | F103 (capture scripts in `scripts/`) | open |
| 4 | GDMA descriptor-walk mem-to-mem (ESP32-S3) | `fa292bd` | JTAG Unity run on the bench S3: same descriptor sequence on silicon, byte-compare (recipe: `HW_ORACLE_RESULT.md` in the platformio demo) | bench ESP32-S3 (proven setup) | open |
| 5 | ESP32-C3 TIMG0 wired to the real Timg model | `9dfe444` | T0 counter advance + latch semantics on silicon (JTAG or UART-reporting probe firmware) | **ESP32-C3 board availability unconfirmed** — blocked on hardware until then | open (blocked-on-HW) |
| 6 | demo-blinky pacing change (#86) vs stored hardware traces | `d360785` | Nightly `Trace Drift Assertions` fails since #86: stored silicon traces were captured with the old 2M-NOP firmware. Re-capture demo-blinky traces on the F103 bench (`scripts/hw-capture-stm32f103.sh`, `diff-sim-vs-hw.sh`) or pin traces to firmware hash | F103 bench | open — **causes a red nightly job today** |

Notes:
- Recipes reuse existing machinery only: hw-oracle mmio-diff replays, `thumb_oracles`,
  the S3 JTAG Unity loop, F103 capture scripts. No new harnesses required.
- Entry #6 is the live example of the rule working: a firmware-side change tripped
  the silicon comparison the same day it merged.
