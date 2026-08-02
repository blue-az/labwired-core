// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! ESP32-C3 Bluetooth LE link-layer / baseband block (`0x6003_1000`, 4 KiB) —
//! behavioral model, built the same way as [`super::wifi_mac`].
//!
//! **There is no datasheet for this window.** Espressif does not publish the
//! WiFi/BT MAC registers, and the ESP32-C3 TRM stops at the crypto estate.
//! Everything below was reverse-engineered from the connected ESP32-C3
//! SuperMini (MAC `9c:cc:01:d0:5e:70`, built-in USB-JTAG) running an Arduino
//! `BLEDevice::init()` + `startAdvertising()` probe — **silicon capture
//! 2026-08-02**. Where a value could not be determined from silicon it is
//! called out in a comment rather than invented; a model that lies is worse
//! than a fault.
//!
//! ## How the window was mapped
//!
//! Three OpenOCD passes against the live part (`board/esp32c3-builtin.cfg`):
//!
//! 1. **Idle dumps.** `reset halt` reads the whole window as `00000000` — the
//!    block is clock-gated out of reset. After `BLEDevice::init()` it is dense
//!    with state, and the control window `i2s0` (`0x6002_D000`) still reads
//!    zero, so the non-zero reads are a real block and not bus float.
//! 2. **Write trace.** A `wp 0x60031000 0x1000 w` watchpoint from `reset halt`,
//!    resuming into 400 hits while capturing PC + all 31 GPRs each time. The
//!    RISC-V store at the trapping PC (the trigger fires *before* the store)
//!    was then decoded offline to recover the exact target offset and value.
//!    303 hits covered the whole of BLE bring-up; the watchpoint then went
//!    quiet, i.e. that IS the init write sequence.
//! 3. **Read trace.** The same with `wp ... r`, to find busy-wait polls.
//!
//! ## What the traces proved
//!
//! **Almost all of BLE bring-up is register-backed read-modify-write.** The
//! read trace found *no* status-bit spin loop anywhere in `BLEDevice::init()`:
//! every repeated `(pc, offset)` pair is a read-modify-write or a table walk
//! (e.g. `+0x2C4` re-read ten times, once per ROM-patch slot, each time OR-ing
//! in one more enable bit and writing it straight back). So plain storage with
//! a zero reset — exactly what silicon reads while gated — carries the
//! controller through init, and that is what this model gives the whole window.
//!
//! On top of that, exactly one thing needs *real behaviour*:
//!
//! * **`+0x01C` — the Bluetooth native clock (CLKN), read/write asymmetric.**
//!   The BT ROM routine at `0x4002_EE60` does: read `+0x01C` → compute a
//!   deadline → **write** `+0x01C` with `0x8000_0000 | target` (`0x4002_EE72`)
//!   → **re-read** `+0x01C` twice (`0x4002_EE78`, `0x4002_EE8E`) → read
//!   `+0x020` (`0x4002_EE92`). The write must not become what the read
//!   returns, so the model keeps them separate.
//!
//!   **The top bit is now settled** (silicon capture 2026-08-02, board
//!   `38:44:be:42:f5:58`): that routine is `r_rwip_time_get`, and the mask ROM
//!   — read back off this part and matched word-for-word against the symbolled
//!   `esp32c3_rev3_rom.elf` — does `+0x01C |= 0x8000_0000`, then
//!   **`lw a5,28(s0); bltz a5, -4`**, i.e. spins while the read-back is still
//!   negative, and only then samples `+0x01C` and `+0x020`. So bit31 is a
//!   *sample-latch request* the hardware clears when a coherent
//!   `BASETIMECNT`/`FINETIMECNT` pair is ready — the earlier "sample/latch vs.
//!   next-event comparator" ambiguity resolves to the former, and the
//!   comparators live at `+0x0E4`/`+0x0E8`/`+0x0EC` instead (see below).
//!   Keeping the write out of the read, which this model already did, is
//!   exactly what the handshake needs: the sample is always ready, so the spin
//!   exits at once. The written value is still kept in
//!   [`Esp32c3Bt::armed_event_target`] for inspection.
//!
//!   This is the one register a frozen value would deadlock: the routine
//!   schedules the *next* event off the *current* clock and re-reads to check
//!   whether its deadline already slipped, so a constant makes the controller
//!   either spin or re-arm the same instant forever.
//!
//! * **`+0x020` — the sub-tick fine counter** the same routine reads right
//!   after CLKN, for sub-CLKN resolution.
//!
//! ## Timebase provenance (silicon capture 2026-08-02)
//!
//! `+0x01C` was sampled across known wall-clock intervals with the part
//! advertising (`halt; mdw; resume; sleep N`):
//!
//! | interval | delta | rate |
//! |---|---|---|
//! | 1 s | `0x3e938c → 0x3ea0d0` = 3396 | ~3396 Hz |
//! | 2 s | `0x3ea0d0 → 0x3eba6f` = 6559 | ~3280 Hz |
//!
//! (Both are slight over-reads: each sample pays halt/resume overhead on top
//! of the sleep.) That lands on the **Bluetooth native clock, which ticks
//! every 312.5 µs = 3200 Hz** — a spec-defined period, not a fitted constant.
//! `+0x020` was never observed at or above 625 over a dozen samples (max
//! `0x236` = 566, mean ≈ 440 — consistent with uniform sampling of `0..624`),
//! and 312.5 µs × 2 MHz = 625 exactly, so it is modelled as a **half-µs
//! counter that wraps once per CLKN tick**. Together the pair is a textbook BT
//! baseband timebase, and both halves are derived from the sim's cycle clock
//! so they stay coherent with device time under idle fast-forward.
//!
//! ## Deliberately NOT modelled (say so rather than invent)
//!
//! * **`+0x024`** is live on silicon but is **not monotonic** — sampled values
//!   `0x1000, 0x1050, 0x1064, 0x108c, 0x10a0, 0x10b4` include decreases, so it
//!   is not a counter. Every observed value is `0x1000 + 20·n`, which hints at
//!   a rotating slot index, but that is a guess and the read trace shows
//!   `BLEDevice::init()` never reads it. It stays plain storage; if a firmware
//!   ever polls it the run will stall visibly rather than be lied to.
//! * **The radio itself.** As with the WiFi MAC, there is no RF here. This is
//!   an air-gapped behavioral endpoint, not a faithful BLE PHY.
//!
//! ## The interrupt path (silicon capture 2026-08-02, board `38:44:be:42:f5:58`)
//!
//! **Routing.** Interrupt-matrix dump after init: `RWBLE_IRQ_MAP`
//! (`0x600C_2020`, matrix source 8) = 5 and `BT_BB_INT_MAP` (`0x600C_2014`,
//! source 5) = 8. So the RW-BLE core drives **matrix source 8**, which the
//! firmware routes to CPU line 5. That is the source this model exports.
//!
//! **Where the bit meanings come from — read the ROM, do not guess.** The
//! previous pass could name `INTCNTL`/`INTSTAT`/`INTRAWSTAT`/`INTACK` but not
//! say which bit is which event, and refused to invent them. They are now
//! settled from two independent silicon-anchored sources:
//!
//! 1. **The mask ROM's own dispatcher.** `esp32c3_rev3_rom.elf` (shipped in
//!    `esp-rom-elfs`) carries full symbols for the RW-BLE stack, and the bytes
//!    at `r_rwble_isr` (`0x4002_E64A`) were read back off this part over JTAG
//!    and match the ELF word for word (`7179 ce4e 3fce09b7 …`), so the
//!    disassembly IS this silicon. `r_rwble_isr` and `r_rwip_isr`
//!    (`0x4002_F4EC`) test the status word bit by bit; each arm W1C-writes
//!    exactly its own bit to `+0x018` and calls one entry of a dispatch table.
//! 2. **The live dispatch tables.** `r_modules_funcs_p` (`0x3FCD_FF88`) and
//!    `r_ip_funcs_p` (`0x3FCD_FF8C`) were dereferenced on the running,
//!    advertising part (`0x3FC9_C024` / `0x3FC9_C404`) and dumped. All but
//!    three of the slots the two ISRs index hold ROM thunks that resolve
//!    straight to a name; the three that ESP-IDF patches into IRAM are handled
//!    below and marked.
//!
//! That yields, bit → handler:
//!
//! | bit | mask | handler (from the live table) |
//! |---|---|---|
//! | 0  | `0x00000001` | `r_rwip_wakeup_end` † (gated on `rwip_env+0x16` bit 0) |
//! | 1  | `0x00000002` | `r_sch_prog_tx_isr` |
//! | 2  | `0x00000004` | `r_sch_prog_rx_isr` |
//! | 3  | `0x00000008` | `r_rwip_wakeup` † |
//! | 5  | `0x00000020` | `r_sch_prog_end_isr` † (end of a programmed radio event) |
//! | 6  | `0x00000040` | `r_sch_prog_skip_isr` |
//! | 7  | `0x00000080` | `r_rwip_crypt_isr_handler` |
//! | 8  | `0x00000100` | error: dumps `DIAG0/1`, `EM BASE ERROR`, `FSMERROR`; its slot is NULL on this build, so the ISR only asserts (`rwble.c 261`) |
//! | 9  | `0x00000200` | `r_rwip_timer_10ms_handler` |
//! | 10 | `0x00000400` | (armed by `r_rwip_timer_hs_set`; no handler slot — the arm/ack sequence is the whole evidence) |
//! | 11 | `0x00000800` | `r_rwip_timer_hus_handler` |
//! | 12 | `0x00001000` | `r_rwip_sw_int_handler` |
//! | 18 | `0x00040000` | `r_lld_update_rxbuf_isr` |
//! | 19 | `0x00080000` | `r_ble_sw_cca_check_isr` |
//! | 21 | `0x00200000` | `IRQ FIFO ALMOST FULL:cnt %u, rem %u` (ROM string) |
//! | 22 | `0x00400000` | fatal: sets `+0x2D8` bit31, acks `0x7FFFFF`, asserts |
//!
//! † These three slots hold IRAM addresses on the live part — ESP-IDF's
//! `libble_app` patches them — so the live table alone leaves them unnamed.
//! They are named by *slot position*, which is inference, not measurement, and
//! is flagged as such: both dispatch tables are laid out in the same order as
//! the ROM's own thunk table, so a patched slot's original identity is the
//! thunk its unpatched neighbours skipped. `+1728` sits between
//! `__call_r_sch_prog_init` (`0x4000153C`) and its predecessor `0x40001538` =
//! `__call_r_sch_prog_end_isr`; `+736`/`+740` follow
//! `__call_r_rwip_timer_hus_set` (`0x400014CC`) and take `0x400014D0` =
//! `__call_r_rwip_wakeup` and `0x400014D4` = `__call_r_rwip_wakeup_end`. The
//! method is cross-checked by every unpatched slot in those runs agreeing with
//! its position, and by bit 11's independently-confirmed
//! `r_rwip_timer_hus_handler` (the `+0x0EC` comparator proves that one from
//! the register side). Nothing in the model depends on these three names.
//!
//! Cross-check: the enable word read off the live part is `INTCNTL =
//! 0x0064_0B66`, whose set bits are exactly `{1,2,5,6,8,9,11,18,21,22}` — the
//! subset above that a *controller doing nothing but advertising* needs, with
//! the crypto (7), software (12), CCA (19) and unexplained (0, 3) bits masked
//! off. The bit map and the enable mask were derived independently and agree.
//!
//! **`INTSTAT` is `INTRAWSTAT & INTCNTL`.** Measured across ~20 halts of the
//! advertising part, on every raw value that turned up:
//! `(0x0640B66, 0x011) → 0x000`, `(0x0640B66, 0x811) → 0x800`, and
//! `(0x0640B66, 0x031) → 0x020`. So `+0x010` is derived, not stored — this
//! model computes it and refuses a write to it.
//!
//! **The three comparators.** The ROM's timer setters say which register arms
//! which bit, unambiguously — each writes a target, W1C-acks its own bit, and
//! ORs its enable into `INTCNTL` (and clears that enable to disarm):
//!
//! | setter | target register | unit | bit |
//! |---|---|---|---|
//! | `r_rwip_timer_10ms_set` | `+0x0E4` | 10 ms = 32 CLKN ticks (the setter also keeps `rwip_env+8 = target << 5`, i.e. half-slots) | 9 |
//! | `r_rwip_timer_hs_set`   | `+0x0E8` | half-slot = 1 CLKN tick | 10 |
//! | `r_rwip_timer_hus_set`  | `+0x0EC` (base, 28-bit) + `+0x0F0` (fine, written as `624 - hus`) | CLKN + `FINETIMECNT` | 11 |
//!
//! And the live part confirms the hus comparator is the one driving
//! advertising: sampled four times while advertising, `+0x0EC` always sat
//! 119–150 CLKN ticks *ahead* of `+0x01C` (`0x462F→0x46B1`, `0x50BB→0x5151`,
//! `0x57E8→0x587E`, `0x6810→0x6887`), i.e. 37–47 ms out — a BLE advertising
//! interval with its random delay. `+0x0F0` read `0x270` = 624 (= `624 - 0`).
//!
//! **A masked comparator does not latch.** At the same halts `+0x0E8` held a
//! long-stale `0x91` (CLKN was `0x462F`) with `INTCNTL` bit 10 *clear*, and
//! `INTRAWSTAT` bit 10 read **0**. If the comparator latched raw status
//! regardless of the mask, bit 10 would have been set. So this model runs a
//! comparator only while its `INTCNTL` enable is set — which is also exactly
//! how the ROM arms and disarms them.
//!
//! **The IRQ FIFO at `+0x2D8`.** `sdk_cfg_priv_opts[69]` reads `0x01` on this
//! part, which selects `r_rwble_isr`'s FIFO path over its plain-`INTSTAT`
//! path — so the FIFO is NOT optional, and a model that raises the line
//! without it would spin the ISR forever (it returns without acking when the
//! FIFO is empty). The layout falls straight out of the dispatcher:
//!
//! ```text
//! bit0      write 1 = pop the head entry   (ori a5,a5,1; sw a5,728(a4))
//! bits[4:1] rem  — free slots              (printf "rem %u", (w >> 1) & 15)
//! bits[9:5] cnt  — queued entries          (printf "cnt %u", (w >> 5) & 31)
//! bits[30:10] the head entry's bitmap, in INTSTAT bit positions
//!             (s0 = (w << 1) >> 11, then dispatched with the SAME masks the
//!              plain path applies to a raw INTSTAT read at 0x4002E8EA)
//! bit31     set by the ISR on the bit-22 fatal path
//! ```
//!
//! and the live reads agree exactly, on two different interrupts and on the
//! idle case:
//!
//! | `+0x2D8` | `INTSTAT` | `INTRAWSTAT` | decodes to |
//! |---|---|---|---|
//! | `0x0020_003E` | `0x800` | `0x811` | cnt 1, rem 15, bitmap `0x800` (hus timer) |
//! | `0x0000_803E` | `0x020` | `0x031` | cnt 1, rem 15, bitmap `0x020` (`sch_prog_end`) |
//! | `0x0000_001E` | `0x000` | `0x011` | cnt 0, rem 15, empty |
//!
//! The idle row also pins the empty-FIFO word: `rem` is a 4-bit field and the
//! FIFO is 16 deep, so "15 free" and "16 free" are the same encoding, and
//! silicon reads `0x1E`. That is what this model returns rather than a guess.
//!
//! **`+0x01C` bit31 is a sample-latch handshake, not a comparator.** The
//! earlier pass could not decide; `r_rwip_time_get` settles it —
//! `+0x01C |= 0x8000_0000`, then **spin while the read-back is negative**,
//! then read `+0x01C` and `+0x020`. So the write requests a coherent
//! base/fine latch and the hardware clears the bit when the sample is ready.
//! Keeping the write out of the read (which this model already did) is what
//! that demands: the spin exits as soon as the sample is up. The same routine
//! masks the base counter with `0x0FFF_FFFF` and `r_rwip_timer_hus_set`
//! asserts on a target `& 0xF000_0000`, so **CLKN is 28 bits**, not 31.
//!
//! ## What the interrupt path buys the twin, and where it stops
//!
//! Measured on the ESP32-C3 rom-boot twin running the same Arduino
//! `BLEDevice::init()` + `startAdvertising()` probe image the silicon capture
//! used, 500 M steps (~1.32 G cycles), `LABWIRED_BT_TRACE=1`:
//!
//! * **Before** (no interrupt path): `PRE_BLE / BLE_INIT_OK / ADV_ON / ALIVE`,
//!   322 BT register writes, the last at CLKN 686 (~214 ms of BT time). The
//!   controller armed the hus comparator exactly once (`+0x0EC <= 0x25`),
//!   never got the interrupt, and spent the remaining ~8 s of device time
//!   re-reading `+0x01C` and nothing else.
//! * **After**: the same four markers (no bring-up regression), 372 writes,
//!   and **three real interrupts taken and handled** — hus at CLKN 37, hs at
//!   CLKN 40, hus again at CLKN 688. Each one is followed in the trace by the
//!   exact ROM ISR sequence: pop the FIFO (`+0x2D8 <= 0x0020_003F`), W1C the
//!   bit (`+0x018 <= 0x800`), drop the enable, mirror the ack to `+0x38C`.
//!   Behaviour that never happened before the interrupt path existed: the
//!   handler kicks `RWBLECNTL` (`+0x000 <= 0x0210_070F`, `0x0310_070F`), arms
//!   the **half-slot** comparator (`+0x0E8 <= 0x28` — a register the polled
//!   build never wrote), runs a second scheduler cycle, re-arms the hus
//!   comparator for the next advertising instant (`+0x0EC <= 0x2B0`), and then
//!   programs a radio event (`+0x32C <= 0x4000_000D`, `+0x100 <=
//!   0x8000_0000`).
//!
//! It stops there, and the reason is structural rather than fixable by more
//! register archaeology: having programmed a radio event, the controller waits
//! for `r_sch_prog_end_isr` / `_tx_isr` / `_rx_isr` (bits 5, 1, 2). Those come
//! from an RF block this model does not have and cannot honestly fake — see
//! the WiFi MAC, which is air-gapped for the same reason. Silicon shows that
//! is exactly the missing edge: sampling the advertising part 14 times caught
//! one mid-event, `INTSTAT = 0x20` / `INTRAWSTAT = 0x31` / `+0x2D8 =
//! 0x0000_803E` — a queued **bit 5**, `sch_prog_end`, the end of a radio
//! event. So the twin now gets two full link-layer scheduler cycles and parks
//! at the radio boundary, instead of parking immediately after arming its
//! first timer.
//!
//! ## Interrupt-path gaps still open (say so rather than invent)
//!
//! * **Bits 0 and 4 read raw-set on live silicon** (`INTRAWSTAT = 0x11`, then
//!   `0x811`) and this model never sets them, because nothing here knows what
//!   *raises* them. Both are masked off in `INTCNTL`, so they cannot reach the
//!   CPU either way. Bit 0 at least has a plausible story — its handler slot
//!   is `r_rwip_wakeup_end` and this model has no sleep path — but "which
//!   hardware condition sets the latch" was not measured, so it is not
//!   modelled. Bit 4 has no handler in either ISR at all and no story
//!   whatsoever. Recorded, not faked.
//! * **The radio-event bits (1, 2, 5, 6 — `sch_prog` tx/rx/end/skip, and
//!   18/19)** are *named*, and bit 5 was caught live on silicon, but none of
//!   them is *raised* here: they come from a radio this model does not have.
//!   Only the three timer comparators fire.
//! * **The comparator edge rule** was not measured. This model fires on
//!   "reached or passed" (28-bit wrapping compare) rather than strict
//!   equality, because a strict-equality comparator that misses its instant
//!   would deadlock the controller for a full 28-bit wrap (~23 h of device
//!   time). The two are indistinguishable whenever the deadline is not missed.

use crate::{CycleClock, Peripheral, PeripheralTickResult, SimResult};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

/// Block size. The watchpoint that produced the write trace covered
/// `0x60031000 + 0x1000` and caught every store BLE bring-up made; the highest
/// offset it ever touched is `+0x530`. Silicon reads `0x6003_2000` (and
/// `0x6003_0000`, `0x6002_F000`, `0x6002_E000`) as all-zero with BLE up, so the
/// live block ends before the next window and 4 KiB is the honest extent.
pub const BT_SIZE: u64 = 0x1000;

/// Base address of the block (between `i2s0` at `0x6002_D000` and the WiFi MAC
/// at `0x6003_3000`).
pub const BT_BASE: u64 = 0x6003_1000;

/// `BASETIMECNT` — the Bluetooth native clock. READ = the free-running
/// 312.5 µs counter; WRITE = a control write (`0x8000_0000` set, low bits
/// tracking the clock — see the module docs) that must NOT shadow the read.
const CLKN: u64 = 0x01C;
/// `FINETIMECNT` — sub-CLKN fine counter, `0..=624` at 2 MHz (half-µs), read
/// straight after CLKN by the BT ROM's event scheduler.
const CLKN_FINE: u64 = 0x020;

/// Top bit of a `CLKN` write — the sample-latch request. `r_rwip_time_get`
/// sets it and then spins while the read-back is negative, so the hardware
/// clears it when the coherent base/fine pair is ready. Modelled by never
/// letting the write reach the read: the sample is always ready.
const CLKN_TARGET_ARM: u32 = 0x8000_0000;

/// CLKN is 28 bits: `r_rwip_time_get` masks the counter with this, and
/// `r_rwip_timer_hus_set` asserts on a target with any bit above it set.
const CLKN_MASK: u32 = 0x0FFF_FFFF;

/// `INTCNTL` — interrupt enable mask.
const INTCNTL: u64 = 0x00C;
/// `INTSTAT` — masked status. Derived: `INTRAWSTAT & INTCNTL` (measured).
const INTSTAT: u64 = 0x010;
/// `INTRAWSTAT` — raw status latch.
const INTRAWSTAT: u64 = 0x014;
/// `INTACK` — write-1-to-clear on the raw latch. Reads 0 on silicon.
const INTACK: u64 = 0x018;

/// 10 ms timer comparator target, in units of 32 CLKN ticks
/// (`r_rwip_timer_10ms_set`). Raises bit 9.
const TIMER_10MS_TARGET: u64 = 0x0E4;
/// Half-slot timer comparator target, in CLKN ticks (`r_rwip_timer_hs_set`).
/// Raises bit 10.
const TIMER_HS_TARGET: u64 = 0x0E8;
/// Half-µs timer comparator: base-time target in CLKN ticks
/// (`r_rwip_timer_hus_set`). Paired with [`TIMER_HUS_FINE`]. Raises bit 11.
const TIMER_HUS_TARGET: u64 = 0x0EC;
/// Half-µs timer comparator: fine target, written as `624 - hus`, compared
/// against `FINETIMECNT` directly.
const TIMER_HUS_FINE: u64 = 0x0F0;

/// The IRQ FIFO the ROM ISR reads when `sdk_cfg_priv_opts[69] != 0` (it does,
/// on this silicon). See the module docs for the field layout.
const IRQ_FIFO: u64 = 0x2D8;
/// Secondary `INTACK` alias: the ROM mirrors every ack it writes to `+0x018`
/// here whenever the FIFO path is selected (and the timer *disable* paths
/// write only here). Reads 0 on silicon, like `INTACK`.
const INTACK_FIFO: u64 = 0x38C;

/// Interrupt bits this model can actually raise — the three timer comparators.
const INT_TIMER_10MS: u32 = 1 << 9;
const INT_TIMER_HS: u32 = 1 << 10;
const INT_TIMER_HUS: u32 = 1 << 11;

/// C3 interrupt-matrix source for the RW-BLE core. Silicon capture
/// 2026-08-02: `0x600C_2020` (the source-8 map register) reads 5, i.e. the
/// firmware routes source 8 to CPU line 5.
const RWBLE_IRQ_SOURCE: u32 = 8;

/// Depth of the IRQ FIFO. Silicon read `+0x2D8 = 0x0020_003E` → `cnt = 1`,
/// `rem = 15`, so `rem = depth - cnt` with a depth of 16.
const IRQ_FIFO_DEPTH: u32 = 16;

/// `RWBLECNTL` — the core control word.
const RWBLECNTL: u64 = 0x000;
/// `RWBLECNTL` bit31: a **self-clearing** command bit (the RW-BLE core's
/// master soft-reset / kick). Silicon capture 2026-08-02 attests this directly
/// and twice over:
///
/// * the write trace shows the controller writing `+0x000` in pairs — first
///   the plain control word, then the same word with bit31 set
///   (`0x0010_060f` → `0x8010_060f`, later `0x0010_070f` → `0x8010_070f`);
/// * yet **every** idle dump of a live, advertising part reads `+0x000` back
///   as `0x0010_070f`, i.e. with bit31 CLEAR, even though the last write set
///   it.
///
/// So the hardware consumes and drops the bit. Storing the write verbatim
/// (which is what a plain register-backed window does) wedges the controller:
/// it writes the kick and then spins waiting to read the bit go away. That is
/// exactly where the twin parked before this — the last BT write it ever made
/// was `+0x000 <= 0x8010_070f`, and the CPU sat on the instruction immediately
/// after that store while the real part carried straight on into the next
/// bring-up step.
const RWBLECNTL_SELF_CLEARING: u32 = 0x8000_0000;

/// Read-only hardware identity/configuration words, seeded from the silicon
/// capture of 2026-08-02. These are the ONLY snapshot-seeded values in the
/// model; everything else is either derived (the timebase) or plain storage.
///
/// Both are read by controller bring-up and never appear as a store target in
/// the 303-hit write trace, i.e. they are hardwired in the IP, not firmware
/// state — and both read identically across every boot and every session
/// captured.
///
/// `+0x004` is **firmware-attested**, not inferred: with the window mapped but
/// this word left at 0, the controller stops with its own assertion naming the
/// value it demands —
///
/// ```text
/// assert lld.c 318, param 00000000 09001b00
///                        ^read     ^expected
/// ```
///
/// `lld.c` / `llm_adv.c` / `rwble.c` / the `EM_BLE_*_OFFSET` log strings in the
/// app image identify this block as a **RivieraWaves RW-BLE core**, whose
/// register file opens `RWBLECNTL`(+0x00), `VERSION`(+0x04), `RWBLECONF`(+0x08),
/// `INTCNTL`(+0x0C), `INTSTAT`(+0x10), `INTRAWSTAT`(+0x14), `INTACK`(+0x18),
/// `BASETIMECNT`(+0x1C), `FINETIMECNT`(+0x20) — which is exactly the shape the
/// write trace shows (firmware writes +0x0C and W1C-writes +0x18 with the same
/// bits that read back at +0x10/+0x14, and reads/writes +0x1C/+0x20 as the
/// timebase). `+0x008` is `RWBLECONF`, the build-option word the controller
/// sizes its exchange memory from.
const HW_IDENTITY: &[(u64, u32)] = &[
    (0x004, 0x0900_1b00), // VERSION
    (0x008, 0x0f22_d0b0), // RWBLECONF
];

/// CPU cycles per CLKN tick. 312.5 µs at the C3's 160 MHz CPU clock — the same
/// "hardcode against 160 MHz and say so" convention the WiFi MAC's beacon
/// cadence uses (`MEDIUM_BEACON_INTERVAL_CYCLES`). Peripherals are not handed
/// `cpu_hz`, and every C3 system descriptor in-tree runs at 160 MHz.
const CYCLES_PER_CLKN_TICK: u64 = 50_000;
/// Fine-counter ticks per CLKN tick: 312.5 µs × 2 MHz.
const FINE_TICKS_PER_CLKN: u64 = 625;
/// CPU cycles per fine tick (`CYCLES_PER_CLKN_TICK / FINE_TICKS_PER_CLKN`).
const CYCLES_PER_FINE_TICK: u64 = CYCLES_PER_CLKN_TICK / FINE_TICKS_PER_CLKN;

/// Process-cached `LABWIRED_BT_TRACE` gate. Read ONCE per process — the write
/// path is hot and `std::env::var` is a syscall-backed lookup (same reasoning
/// as the WiFi MAC's `rxbuf_trace_enabled`).
fn bt_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("LABWIRED_BT_TRACE").is_ok())
}

#[derive(Debug, Default)]
pub struct Esp32c3Bt {
    /// The whole 4 KiB window as plain storage. Reset 0 — which is what
    /// silicon reads at `reset halt`, because the block is clock-gated until
    /// the controller enables it.
    regs: Vec<u32>,
    /// Last value written to `CLKN` (low 31 bits), and whether the latch bit
    /// was set. Kept out of `regs` because a `CLKN` read must return the
    /// running clock, not this — see the `+0x01C` note in the module docs.
    /// Inspection only: the timer comparators are `+0x0E4`/`+0x0E8`/`+0x0EC`.
    event_target: u32,
    event_armed: bool,
    /// `INTRAWSTAT` (`+0x014`) — the raw interrupt latch. Set by the
    /// comparators, cleared W1C through `INTACK` (`+0x018`) or its `+0x38C`
    /// mirror. `INTSTAT` is derived from this and `INTCNTL`, never stored.
    int_raw: Cell<u32>,
    /// Comparator bits whose target register has actually been programmed.
    ///
    /// Deliberate deviation, called out rather than hidden: the ROM always
    /// writes a target and *then* ORs the enable in, so on silicon an enabled
    /// comparator always has a real deadline. A model that ran the comparison
    /// against an unwritten register would treat the reset value 0 as a
    /// deadline in the past and fire the instant firmware set the enable —
    /// a fabricated interrupt out of a register nobody programmed. So an
    /// un-programmed comparator counts as disarmed.
    comparators_programmed: u32,
    /// Comparator bits that have already latched for their CURRENT arming.
    ///
    /// A comparator fires ONCE per arm. Without this the "reached or passed"
    /// edge rule would re-latch on every tick after the deadline, so the
    /// instant firmware acked the interrupt the model would raise it again —
    /// an interrupt storm, not a periodic event. Cleared when the target is
    /// reprogrammed or the enable is re-asserted, which is exactly what the
    /// ROM's `r_rwip_timer_*_set` do to schedule the next one.
    comparators_fired: Cell<u32>,
    /// The RW-BLE IRQ FIFO (`+0x2D8`): one bitmap per interrupt the hardware
    /// queued, oldest first. `r_rwble_isr` reads the head, pops it with a
    /// bit0 write, and returns without acking anything when `cnt == 0` — so a
    /// raised line with an empty FIFO is an interrupt storm, not progress.
    irq_fifo: RefCell<VecDeque<u32>>,
    /// Last cycle the bus anchored this model to via `sync_to` — the same
    /// `current_cycle` it then turns a `take_scheduled_events` delay into
    /// `current_cycle + 1 + delay` against. Kept so the scheduled deadline is
    /// exact rather than off by however far the published `CycleClock` lags
    /// mid-batch.
    sync_cycle: Cell<u64>,
    /// Generation stamp for the in-flight scheduled comparator event. Bumped
    /// on every write that could re-arm, so an event scheduled under an older
    /// deadline dies on arrival instead of firing a stale interrupt.
    arm_seq: u32,
    /// Cycle at which the block was first written, i.e. when the controller
    /// un-gated it. CLKN counts from here, so a read before any BLE activity
    /// returns 0 exactly like silicon at `reset halt`. `None` until then.
    clock_base: Option<u64>,
    /// Bus-published cycle clock, attached by
    /// [`SystemBus::add_peripheral`](crate::bus::SystemBus). Drives CLKN and
    /// the fine counter. Not serialized — re-attached by the bus.
    clock: Option<CycleClock>,
}

impl Esp32c3Bt {
    pub fn new() -> Self {
        Self {
            regs: vec![0u32; (BT_SIZE / 4) as usize],
            event_target: 0,
            event_armed: false,
            int_raw: Cell::new(0),
            comparators_programmed: 0,
            comparators_fired: Cell::new(0),
            irq_fifo: RefCell::new(VecDeque::new()),
            sync_cycle: Cell::new(0),
            arm_seq: 0,
            clock_base: None,
            clock: None,
        }
    }

    /// Cycles elapsed since the controller un-gated the block, or 0 while it is
    /// still gated (nothing written yet).
    #[inline]
    fn elapsed_cycles(&self) -> u64 {
        match (self.clock.as_ref(), self.clock_base) {
            (Some(c), Some(base)) => c.now().saturating_sub(base),
            _ => 0,
        }
    }

    /// Bluetooth native clock: one tick per 312.5 µs. 31 bits — the top bit of
    /// `CLKN` is the comparator arm on the write side, and every read captured
    /// from silicon had it clear.
    #[inline]
    fn clkn(&self) -> u32 {
        Self::clkn_at(self.elapsed_cycles())
    }

    #[inline]
    fn clkn_at(elapsed: u64) -> u32 {
        ((elapsed / CYCLES_PER_CLKN_TICK) as u32) & CLKN_MASK
    }

    /// Sub-CLKN fine counter: a half-µs **down** counter, 624 → 0 across each
    /// CLKN tick.
    ///
    /// The direction is not guesswork and it is not the direction this model
    /// first assumed. `r_rwip_time_get` — the ROM routine that samples the
    /// timebase — returns the pair as `(BASETIMECNT & 0x0FFF_FFFF,
    /// 624 - FINETIMECNT)`, and `r_rwip_timer_hus_set(hs, hus)` writes
    /// `624 - hus` into the comparator at `+0x0F0`. Both only make sense if
    /// `FINETIMECNT` runs *down*: the driver's `hus` (half-µs elapsed within
    /// the half-slot, which must increase for `rwip_time_t` arithmetic to
    /// work) is `624 - FINETIMECNT`, and arming "fire `hus` into the target
    /// half-slot" then reduces to the direct register compare
    /// `FINETIMECNT <= +0x0F0`. Direct sampling could NOT settle this — a
    /// dozen JTAG reads land uniformly in `0..624` either way (max seen 566)
    /// — so the ROM's own arithmetic is the evidence. It also matches the
    /// RW-BLE core's `FINECNT` elsewhere in the RivieraWaves family, which is
    /// corroboration, not measurement.
    #[inline]
    fn clkn_fine(&self) -> u32 {
        if self.clock_base.is_none() {
            // Still clock-gated: silicon reads the whole window as zero.
            return 0;
        }
        Self::fine_at(self.elapsed_cycles())
    }

    #[inline]
    fn fine_at(elapsed: u64) -> u32 {
        let within = (elapsed / CYCLES_PER_FINE_TICK) % FINE_TICKS_PER_CLKN;
        (FINE_TICKS_PER_CLKN - 1 - within) as u32
    }

    /// The bus-published cycle count, or 0 before a clock is attached.
    #[inline]
    fn clock_now(&self) -> u64 {
        self.clock.as_ref().map(|c| c.now()).unwrap_or(0)
    }

    /// Cycles since the un-gate as of absolute CPU cycle `now`.
    #[inline]
    fn elapsed_at(&self, now: u64) -> u64 {
        now.saturating_sub(self.clock_base.unwrap_or(now))
    }

    /// True once the bus has handed over a cycle clock and the event-scheduler
    /// build is active — the same predicate `ledc`/`i2c0` use. Without a clock
    /// (feature off, hand-built bus, `force_legacy_walk`) the model stays on
    /// the legacy per-cycle walk so those callers keep the old semantics.
    fn scheduler_mode(&self) -> bool {
        cfg!(feature = "event-scheduler") && self.clock.is_some()
    }

    /// Test/differential knob mirroring `Esp32c3Ledc::force_legacy_walk`: drop
    /// the cycle clock so the model runs the legacy walk instead.
    pub fn force_legacy_walk(&mut self) {
        self.clock = None;
    }

    /// The comparator target last armed via a `CLKN` write, if armed. Exposed
    /// for the interrupt follow-up and for tests.
    pub fn armed_event_target(&self) -> Option<u32> {
        self.event_armed.then_some(self.event_target)
    }

    #[inline]
    fn reg(&self, offset: u64) -> u32 {
        *self.regs.get((offset / 4) as usize).unwrap_or(&0)
    }

    /// `INTCNTL` — the enable mask. Also the arm/disarm switch for the three
    /// timer comparators: the ROM's setters OR their bit in after writing the
    /// target and AND it out to disarm.
    #[inline]
    fn int_enable(&self) -> u32 {
        self.reg(INTCNTL)
    }

    /// `INTSTAT` (`+0x010`) — derived, never stored. Silicon:
    /// `0x0640B66 & 0x811 = 0x800`, `0x0640B66 & 0x011 = 0x000`.
    #[inline]
    fn int_status(&self) -> u32 {
        self.int_raw.get() & self.int_enable()
    }

    /// Absolute cycle (since the un-gate) at which comparator `bit` fires, or
    /// `None` when it is not armed — not programmed, or its enable is clear.
    ///
    /// Worked in the elapsed-cycle domain rather than by comparing CLKN
    /// values, so "already past" needs no wrap arithmetic. The model therefore
    /// does NOT handle the 28-bit CLKN wrap, which is ~23 hours of device time
    /// — stated rather than pretended away.
    fn deadline_cycles(&self, bit: u32) -> Option<u64> {
        if self.int_enable() & self.comparators_programmed & bit == 0 {
            return None;
        }
        Some(match bit {
            // +0x0E4 counts in 10 ms units = 32 CLKN ticks (the setter keeps
            // `rwip_env+8 = target << 5` half-slots alongside it).
            INT_TIMER_10MS => u64::from(self.reg(TIMER_10MS_TARGET)) * 32 * CYCLES_PER_CLKN_TICK,
            // +0x0E8 counts in half-slots = CLKN ticks.
            INT_TIMER_HS => u64::from(self.reg(TIMER_HS_TARGET)) * CYCLES_PER_CLKN_TICK,
            // +0x0EC is the CLKN target; +0x0F0 holds `624 - hus`, compared
            // against the DOWN-counting FINETIMECNT, so the offset into the
            // target tick is `624 - target` fine ticks.
            INT_TIMER_HUS => {
                let fine_target =
                    u64::from(self.reg(TIMER_HUS_FINE) & 0xFFFF).min(FINE_TICKS_PER_CLKN - 1);
                u64::from(self.reg(TIMER_HUS_TARGET) & CLKN_MASK) * CYCLES_PER_CLKN_TICK
                    + (FINE_TICKS_PER_CLKN - 1 - fine_target) * CYCLES_PER_FINE_TICK
            }
            _ => return None,
        })
    }

    /// Comparator bits due at `elapsed` that have not already latched for
    /// their current arming. A comparator fires ONCE per arm.
    fn expired_at(&self, elapsed: u64) -> u32 {
        if self.clock_base.is_none() {
            return 0;
        }
        let spent = self.comparators_fired.get() | self.int_raw.get();
        let mut fired = 0;
        for bit in [INT_TIMER_10MS, INT_TIMER_HS, INT_TIMER_HUS] {
            if spent & bit != 0 {
                continue;
            }
            if matches!(self.deadline_cycles(bit), Some(d) if elapsed >= d) {
                fired |= bit;
            }
        }
        fired
    }

    /// Latch every comparator due at `elapsed` into `INTRAWSTAT` and queue one
    /// IRQ FIFO entry per rising edge. `&self` (via `Cell`/`RefCell`) so a
    /// read or a level poll can materialise a deadline that has just come due
    /// without waiting for the next event — the `ledc` `sync_from_clock`
    /// pattern.
    fn latch_at(&self, elapsed: u64) -> bool {
        let rising = self.expired_at(elapsed);
        if rising == 0 {
            return false;
        }
        self.int_raw.set(self.int_raw.get() | rising);
        self.comparators_fired
            .set(self.comparators_fired.get() | rising);
        // The FIFO carries the bits the ISR is meant to dispatch, i.e. the
        // enabled ones (`r_rwble_isr` feeds the head bitmap through the same
        // masks it applies to a raw INTSTAT read).
        let queued = rising & self.int_enable();
        let mut fifo = self.irq_fifo.borrow_mut();
        if queued != 0 && (fifo.len() as u32) < IRQ_FIFO_DEPTH {
            fifo.push_back(queued);
        }
        if bt_trace_enabled() {
            eprintln!(
                "[bt] IRQ raw|={rising:#010x} stat={:#010x} fifo_cnt={} (clkn={} fine={})",
                self.int_raw.get() & self.int_enable(),
                fifo.len(),
                Self::clkn_at(elapsed),
                Self::fine_at(elapsed)
            );
        }
        true
    }

    /// Materialise any comparator that has come due as of the bus-published
    /// clock. Cheap no-op while nothing is armed.
    fn sync_from_clock(&self) {
        if self.clock_base.is_some() {
            self.latch_at(self.elapsed_cycles());
        }
    }

    /// Cycles from `elapsed` to the nearest armed, unspent comparator, or
    /// `None` when nothing is scheduled. Zero when one is already due.
    fn cycles_to_next_deadline(&self, elapsed: u64) -> Option<u64> {
        let spent = self.comparators_fired.get() | self.int_raw.get();
        [INT_TIMER_10MS, INT_TIMER_HS, INT_TIMER_HUS]
            .into_iter()
            .filter(|bit| spent & bit == 0)
            .filter_map(|bit| self.deadline_cycles(bit))
            .map(|deadline| deadline.saturating_sub(elapsed))
            .min()
    }

    /// `+0x2D8` read: `cnt`/`rem` plus the head entry's bitmap, in the exact
    /// field positions `r_rwble_isr` decodes.
    fn irq_fifo_word(&self) -> u32 {
        let fifo = self.irq_fifo.borrow();
        let cnt = (fifo.len() as u32).min(31);
        // `rem` is a 4-bit field and the FIFO is 16 deep, so "15 free" and
        // "16 free" share an encoding — silicon reads `+0x2D8 = 0x0000_001E`
        // (cnt 0, rem 15) while idle, which is what this clamp reproduces.
        // Only `cnt` gates the ISR; `rem` reaches nothing but a log string.
        let rem = IRQ_FIFO_DEPTH.saturating_sub(cnt).min(15);
        let head = fifo.front().copied().unwrap_or(0) & 0x001F_FFFF;
        (rem << 1) | (cnt << 5) | (head << 10)
    }

    /// True while a comparator is armed or the line is asserted — the only
    /// states in which the per-cycle walk has anything to do.
    fn irq_work_pending(&self) -> bool {
        self.int_status() != 0
            || (self.clock_base.is_some()
                && self.int_enable()
                    & self.comparators_programmed
                    & !(self.comparators_fired.get() | self.int_raw.get())
                    != 0)
    }
}

impl Peripheral for Esp32c3Bt {
    /// Walk-free: once the bus hands over a cycle clock the comparators ride
    /// scheduled events (`take_scheduled_events` / `on_event`), so each
    /// deadline lands on its exact cycle and the walk has nothing to do. The
    /// C3 walk-free campaign's `EXPECTED_PINNERS` gate is the reason this is
    /// not simply left on the walk. Without a clock the walk does the real
    /// work and the conservative `true` stands.
    fn needs_legacy_walk(&self) -> bool {
        !self.scheduler_mode()
    }

    /// Only tick while a comparator is armed or the line is up — an un-gated
    /// block with nothing scheduled costs nothing.
    fn legacy_tick_active(&self) -> bool {
        self.irq_work_pending()
    }

    /// `INTCNTL` writes arm and disarm the comparators, and an `INTACK` write
    /// drops the level, so walk membership changes outside `tick()`.
    fn legacy_tick_dynamic(&self) -> bool {
        true
    }

    /// Legacy per-cycle drive: expire the timer comparators and hold the
    /// RW-BLE matrix source up while `INTSTAT` is non-zero. Level-sensitive,
    /// like the WiFi MAC's — it stays asserted until firmware W1C-acks through
    /// `+0x018`. In scheduler mode the walk skips this model entirely and the
    /// same latch happens in `on_event`, at the exact deadline cycle.
    fn tick(&mut self) -> PeripheralTickResult {
        self.sync_from_clock();
        PeripheralTickResult {
            explicit_irqs: (self.int_status() != 0).then(|| vec![RWBLE_IRQ_SOURCE]),
            ..Default::default()
        }
    }

    fn uses_scheduler(&self) -> bool {
        self.scheduler_mode()
    }

    /// Anchor the comparators to `now_cycle` before every MMIO write, so a
    /// firmware ack or re-arm observes a deadline that has just come due.
    fn sync_to(&mut self, now_cycle: u64) {
        if !self.scheduler_mode() {
            return;
        }
        self.sync_cycle.set(now_cycle);
        if self.clock_base.is_some() {
            self.latch_at(self.elapsed_at(now_cycle));
        }
    }

    /// Arm the nearest comparator deadline as a single in-flight event, under
    /// a fresh generation so an event scheduled against an older target dies
    /// on arrival. The `- 1` mirrors `ledc`: the bus turns a write-path delay
    /// into the absolute deadline `anchor + 1 + delay`.
    fn take_scheduled_events(&mut self) -> Vec<(u64, u32)> {
        if !self.scheduler_mode() || self.clock_base.is_none() {
            return Vec::new();
        }
        self.arm_seq = self.arm_seq.wrapping_add(1);
        // Anchored on the bus's `current_cycle` (handed over by `sync_to` just
        // before this write), not on the published clock, so the deadline the
        // bus builds as `current_cycle + 1 + delay` lands on the exact cycle.
        let anchor = self.sync_cycle.get().max(self.clock_now());
        match self.cycles_to_next_deadline(self.elapsed_at(anchor)) {
            Some(cycles) => vec![(cycles.saturating_sub(1), self.arm_seq)],
            None => Vec::new(),
        }
    }

    /// Fire the comparator this event was scheduled for at its exact cycle,
    /// then chain to the next armed one. The bus re-derives the matrix source
    /// from [`Self::matrix_irq_sources_into`] after this handler, so the level
    /// goes up here and stays up until firmware acks.
    fn on_event(
        &mut self,
        event_token: u32,
        sched: &mut crate::sched::EventScheduler,
        _bus: &mut dyn crate::Bus,
    ) -> crate::sched::EventResult {
        if !self.scheduler_mode() || event_token != self.arm_seq {
            // Stale chain (re-armed since this event was scheduled): die.
            return crate::sched::EventResult::default();
        }
        let elapsed = self.elapsed_at(sched.now());
        self.latch_at(elapsed);
        crate::sched::EventResult {
            reschedule_delay: self.cycles_to_next_deadline(elapsed),
            ..Default::default()
        }
    }

    /// The live level for the walk-free re-derivation path, same condition as
    /// [`Self::tick`]. Syncs first so a deadline that has just come due is
    /// reflected even between events.
    fn matrix_irq_sources_into(&self, out: &mut Vec<u32>) {
        self.sync_from_clock();
        if self.int_status() != 0 {
            out.push(RWBLE_IRQ_SOURCE);
        }
    }

    fn read(&self, offset: u64) -> SimResult<u8> {
        let w = self.read_u32(offset & !3)?;
        Ok((w >> ((offset & 3) * 8)) as u8)
    }

    fn write(&mut self, offset: u64, value: u8) -> SimResult<()> {
        let aligned = offset & !3;
        let sh = (offset & 3) * 8;
        let cur = *self.regs.get((aligned / 4) as usize).unwrap_or(&0);
        self.write_u32(aligned, (cur & !(0xFFu32 << sh)) | ((value as u32) << sh))
    }

    fn read_u32(&self, offset: u64) -> SimResult<u32> {
        // Materialise any comparator that has come due, so a poll of
        // INTSTAT/INTRAWSTAT/+0x2D8 between scheduled events is not stale.
        self.sync_from_clock();
        if let Some((_, v)) = HW_IDENTITY.iter().find(|(o, _)| *o == offset) {
            // NOTE (deliberate, documented deviation): silicon reads these as 0
            // while the block is still clock-gated, and we do not model that
            // gate — it lives in SYSTEM/APB_CTRL, not in this window. So they
            // read their hardwired value from cycle 0 rather than from BT
            // enable. The asymmetry is on purpose: reading the ID too early
            // harms nothing (no firmware reads it before enabling BT), while
            // reading 0 too late is a hard controller assert. The counters
            // below keep the honest gate, because they demonstrably restart at
            // BT enable (a `reset halt; resume; sleep 3000` capture read CLKN
            // as 0x799 ≈ 0.6 s, not the 3 s since reset).
            return Ok(*v);
        }
        Ok(match offset {
            CLKN => self.clkn(),
            CLKN_FINE => self.clkn_fine(),
            // Derived, not stored: silicon reads INTSTAT == INTRAWSTAT & INTCNTL.
            INTSTAT => self.int_status(),
            INTRAWSTAT => self.int_raw.get(),
            // W1C registers read back 0 on a live advertising part.
            INTACK | INTACK_FIFO => 0,
            IRQ_FIFO => self.irq_fifo_word(),
            _ => self.reg(offset),
        })
    }

    fn write_u32(&mut self, offset: u64, value: u32) -> SimResult<()> {
        // `LABWIRED_BT_TRACE=1` mirrors the WiFi MAC's `LABWIRED_MAC_TRACE`:
        // dump the controller's register programming so a stall can be read
        // straight off the tail of the log and compared with the OpenOCD write
        // trace this model was built from.
        if bt_trace_enabled() {
            eprintln!(
                "[bt] +{offset:#05x} <= {value:#010x}  (clkn={} fine={})",
                self.clkn(),
                self.clkn_fine()
            );
        }
        // First touch of the block = the controller un-gated it; start CLKN
        // here so reads before BLE bring-up stay 0 like gated silicon.
        if self.clock_base.is_none() {
            self.clock_base = Some(self.clock.as_ref().map(|c| c.now()).unwrap_or(0));
        }
        match offset {
            // Arm the next-event comparator. Deliberately NOT stored into
            // `regs`: a subsequent read of this offset must return the running
            // clock (silicon capture 2026-08-02 — the BT ROM writes
            // 0x8000_xxxx here and immediately reads back a plain counter).
            CLKN => {
                self.event_armed = value & CLKN_TARGET_ARM != 0;
                self.event_target = value & !CLKN_TARGET_ARM;
            }
            // Re-asserting an enable re-arms that comparator: `r_rwip_timer_*_set`
            // finishes by OR-ing its bit back into INTCNTL, and the disable
            // paths AND it out. Only the 0->1 edge re-arms, so an unrelated
            // read-modify-write of INTCNTL does not resurrect a spent one.
            INTCNTL => {
                let rearmed = value & !self.int_enable();
                self.comparators_fired
                    .set(self.comparators_fired.get() & !rearmed);
                if let Some(slot) = self.regs.get_mut((offset / 4) as usize) {
                    *slot = value;
                }
            }
            // Consume the self-clearing command bit — the hardware executes it
            // and drops it, so it must never read back set.
            RWBLECNTL => {
                if let Some(slot) = self.regs.get_mut((offset / 4) as usize) {
                    *slot = value & !RWBLECNTL_SELF_CLEARING;
                }
            }
            // W1C on the raw latch. `+0x38C` is the mirror the ROM writes
            // alongside (and, on the timer-disable paths, instead of) `+0x018`.
            INTACK | INTACK_FIFO => {
                self.int_raw.set(self.int_raw.get() & !value);
            }
            // `INTSTAT` is derived and `INTRAWSTAT` is driven by the hardware:
            // neither is a storage slot, so a write to them is dropped rather
            // than allowed to shadow the derivation.
            INTSTAT | INTRAWSTAT => {}
            // Bit 0 pops the head entry. Bit 31 is the ISR's fatal-path flag;
            // it is stored nowhere because nothing here reads it back.
            IRQ_FIFO => {
                if value & 1 != 0 {
                    self.irq_fifo.borrow_mut().pop_front();
                }
            }
            // Programming a target arms its comparator (the enable still has
            // to be set — the ROM ORs it in immediately after).
            TIMER_10MS_TARGET | TIMER_HS_TARGET | TIMER_HUS_TARGET => {
                let bit = match offset {
                    TIMER_10MS_TARGET => INT_TIMER_10MS,
                    TIMER_HS_TARGET => INT_TIMER_HS,
                    _ => INT_TIMER_HUS,
                };
                self.comparators_programmed |= bit;
                self.comparators_fired
                    .set(self.comparators_fired.get() & !bit);
                if let Some(slot) = self.regs.get_mut((offset / 4) as usize) {
                    *slot = value;
                }
            }
            _ => {
                if let Some(slot) = self.regs.get_mut((offset / 4) as usize) {
                    *slot = value;
                }
            }
        }
        Ok(())
    }

    fn attach_cycle_clock(&mut self, clock: CycleClock) {
        self.clock = Some(clock);
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Silicon capture 2026-08-02: the whole window reads `00000000` at
    /// `reset halt` (clock-gated), so an untouched model must too — everywhere
    /// except the two hardwired identity words, which are deliberately always
    /// readable (see the note in `read_u32`).
    #[test]
    fn gated_window_reads_zero() {
        let bt = Esp32c3Bt::new();
        for off in [0x000u64, 0x01C, 0x020, 0x024, 0x204, 0x2C4, 0x370, 0x530] {
            assert_eq!(bt.read_u32(off).unwrap(), 0, "offset {off:#05x} at reset");
        }
    }

    /// `RWBLECNTL` bit31 is a self-clearing command bit: the controller writes
    /// the control word, then writes it again with bit31 set as a kick, and
    /// spins until the bit reads back clear. Silicon reads `+0x000` as
    /// `0x0010_070f` (bit31 clear) on a live part whose last write was
    /// `0x8010_070f`. Regression for the stall that pinned the twin on the
    /// instruction after that store.
    #[test]
    fn rwblecntl_command_bit_self_clears() {
        let mut bt = Esp32c3Bt::new();
        bt.write_u32(RWBLECNTL, 0x0010_070f).unwrap();
        assert_eq!(bt.read_u32(RWBLECNTL).unwrap(), 0x0010_070f);
        bt.write_u32(RWBLECNTL, 0x8010_070f).unwrap();
        assert_eq!(
            bt.read_u32(RWBLECNTL).unwrap(),
            0x0010_070f,
            "bit31 must be consumed, not stored — otherwise the controller \
             spins forever waiting for its own kick to clear"
        );
    }

    /// The controller validates `VERSION` during `lld` bring-up and asserts on
    /// a mismatch, quoting the value it wants:
    /// `assert lld.c 318, param 00000000 09001b00`. Regression for that stop.
    #[test]
    fn hardware_identity_words_read_their_silicon_values() {
        let mut bt = Esp32c3Bt::new();
        assert_eq!(bt.read_u32(0x004).unwrap(), 0x0900_1b00, "VERSION");
        assert_eq!(bt.read_u32(0x008).unwrap(), 0x0f22_d0b0, "RWBLECONF");
        // Read-only: a stray write must not be able to break the assert.
        bt.write_u32(0x004, 0xdead_beef).unwrap();
        bt.write_u32(0x008, 0xdead_beef).unwrap();
        assert_eq!(bt.read_u32(0x004).unwrap(), 0x0900_1b00, "VERSION is RO");
        assert_eq!(bt.read_u32(0x008).unwrap(), 0x0f22_d0b0, "RWBLECONF is RO");
    }

    /// The register-backed majority: BLE bring-up is read-modify-write, so a
    /// written value must read straight back. Values are real ones from the
    /// silicon write trace.
    #[test]
    fn window_is_register_backed() {
        let mut bt = Esp32c3Bt::new();
        for (off, val) in [
            (0x204u64, 0x0002_9725u32), // ROM patch/veneer table entry 0
            (0x2c4, 0x07fe_01ff),       // patch-enable mask, fully populated
            (0x0e0, 0x0190_012c),       // advertising interval pair
            (0x530, 0x0000_0001),
        ] {
            bt.write_u32(off, val).unwrap();
            assert_eq!(bt.read_u32(off).unwrap(), val, "offset {off:#05x}");
        }
    }

    /// `CLKN` is read/write asymmetric: the BT ROM writes a comparator target
    /// with the arm bit and immediately re-reads the *clock*. If the write were
    /// stored the scheduler would read its own deadline back as "now" and
    /// re-arm the same instant forever.
    #[test]
    fn clkn_write_arms_comparator_and_does_not_shadow_the_clock() {
        let mut bt = Esp32c3Bt::new();
        let clock = CycleClock::default();
        bt.attach_cycle_clock(clock.clone());
        clock.publish(0);
        bt.write_u32(CLKN, 0x8000_e8f7).unwrap(); // a real traced value
        assert_eq!(bt.armed_event_target(), Some(0x0000_e8f7));
        assert_eq!(
            bt.read_u32(CLKN).unwrap(),
            0,
            "CLKN read must be the clock, not the armed target"
        );
        // A write without the arm bit disarms.
        bt.write_u32(CLKN, 0x0000_1234).unwrap();
        assert_eq!(bt.armed_event_target(), None);
    }

    /// CLKN advances at the Bluetooth native rate (312.5 µs / 3200 Hz), and the
    /// fine counter wraps `0..=624` once per CLKN tick.
    #[test]
    fn timebase_advances_at_the_bluetooth_native_rate() {
        let mut bt = Esp32c3Bt::new();
        let clock = CycleClock::default();
        bt.attach_cycle_clock(clock.clone());
        clock.publish(1_000); // un-gate at a non-zero cycle
        bt.write_u32(0x000, 0x0010_060f).unwrap();
        assert_eq!(bt.read_u32(CLKN).unwrap(), 0, "CLKN starts at the un-gate");

        // One second of device time at 160 MHz = 3200 CLKN ticks.
        clock.publish(1_000 + 160_000_000);
        assert_eq!(bt.read_u32(CLKN).unwrap(), 3200);

        // Fine counter: half-µs ticks, counting DOWN 624 -> 0 once per CLKN
        // tick (the direction `r_rwip_time_get`'s `624 - FINETIMECNT` and
        // `r_rwip_timer_hus_set`'s `624 - hus` both require).
        clock.publish(1_000);
        assert_eq!(bt.read_u32(CLKN_FINE).unwrap(), 624, "starts full");
        clock.publish(1_000 + CYCLES_PER_FINE_TICK * (624 - 566));
        assert_eq!(bt.read_u32(CLKN_FINE).unwrap(), 566); // max value seen on silicon
        assert_eq!(bt.read_u32(CLKN).unwrap(), 0, "still inside the first tick");
        clock.publish(1_000 + CYCLES_PER_CLKN_TICK);
        assert_eq!(bt.read_u32(CLKN_FINE).unwrap(), 624, "reloads with CLKN");
        assert_eq!(bt.read_u32(CLKN).unwrap(), 1);

        // Never leaves the range silicon showed.
        for n in 0..2_000u64 {
            clock.publish(1_000 + n * 137);
            assert!(bt.read_u32(CLKN_FINE).unwrap() < FINE_TICKS_PER_CLKN as u32);
        }
    }

    /// Bring a model up to the point a live advertising part is at: block
    /// un-gated, the enable word silicon reads, and the hus comparator armed
    /// the way `r_rwip_timer_hus_set` arms it.
    fn advertising_part(clock: &CycleClock) -> Esp32c3Bt {
        let mut bt = Esp32c3Bt::new();
        bt.attach_cycle_clock(clock.clone());
        clock.publish(0);
        bt.write_u32(RWBLECNTL, 0x0010_070f).unwrap(); // un-gate
        bt.write_u32(INTCNTL, 0x0064_0b66).unwrap(); // silicon enable word
        bt
    }

    /// Silicon capture 2026-08-02, board `38:44:be:42:f5:58`: `INTSTAT` is
    /// `INTRAWSTAT & INTCNTL`, not a stored register. Both measured pairs.
    #[test]
    fn int_status_is_raw_and_enable() {
        let clock = CycleClock::default();
        let mut bt = advertising_part(&clock);
        assert_eq!(bt.read_u32(INTCNTL).unwrap(), 0x0064_0b66);

        bt.int_raw.set(0x0000_0011); // measured raw at one halt
        assert_eq!(bt.read_u32(INTSTAT).unwrap(), 0x0000_0000);
        bt.int_raw.set(0x0000_0811); // measured raw at three later halts
        assert_eq!(bt.read_u32(INTSTAT).unwrap(), 0x0000_0800);

        // W1C through INTACK, which itself reads back 0.
        bt.write_u32(INTACK, 0x0000_0800).unwrap();
        assert_eq!(bt.read_u32(INTRAWSTAT).unwrap(), 0x0000_0011);
        assert_eq!(bt.read_u32(INTSTAT).unwrap(), 0);
        assert_eq!(bt.read_u32(INTACK).unwrap(), 0, "INTACK reads 0 on silicon");
        // `+0x38C` is the mirror the ROM writes alongside INTACK.
        bt.int_raw.set(0x0000_0800);
        bt.write_u32(INTACK_FIFO, 0x0000_0800).unwrap();
        assert_eq!(bt.read_u32(INTRAWSTAT).unwrap(), 0);
        assert_eq!(bt.read_u32(INTACK_FIFO).unwrap(), 0);
    }

    /// The half-µs comparator is what drives advertising: `r_rwip_timer_hus_set`
    /// writes the base-time target to `+0x0EC`, the fine target to `+0x0F0`,
    /// acks bit 11 and ORs `0x800` into `INTCNTL`. On the live part `+0x0EC`
    /// sat 119–150 CLKN ticks ahead of `+0x01C` every time it was sampled.
    /// When the timebase gets there the model must raise `INTSTAT` bit 11 and
    /// assert the RW-BLE matrix source — the whole point of this milestone.
    #[test]
    fn hus_comparator_raises_rwble_irq_at_its_target() {
        let clock = CycleClock::default();
        let mut bt = advertising_part(&clock);

        // Arm 130 CLKN ticks out, exactly as the ROM does.
        bt.write_u32(TIMER_HUS_TARGET, 130).unwrap();
        bt.write_u32(TIMER_HUS_FINE, 624).unwrap();
        bt.write_u32(INTACK, INT_TIMER_HUS).unwrap();

        // Not yet: one tick short, the line stays down.
        clock.publish(129 * CYCLES_PER_CLKN_TICK);
        assert!(bt.tick().explicit_irqs.is_none());
        assert_eq!(bt.read_u32(INTSTAT).unwrap(), 0);
        assert!(bt.matrix_irq_sources().is_empty());

        // At the target: raw latches, INTSTAT shows it, matrix source 8 up.
        clock.publish(130 * CYCLES_PER_CLKN_TICK);
        assert_eq!(
            bt.tick().explicit_irqs,
            Some(vec![RWBLE_IRQ_SOURCE]),
            "the hus comparator must raise the RWBLE matrix source"
        );
        assert_eq!(bt.read_u32(INTRAWSTAT).unwrap(), INT_TIMER_HUS);
        assert_eq!(bt.read_u32(INTSTAT).unwrap(), INT_TIMER_HUS);
        assert_eq!(bt.matrix_irq_sources(), vec![RWBLE_IRQ_SOURCE]);

        // Level-sensitive: it stays up until firmware W1C-acks, exactly like
        // the WiFi MAC's event level.
        clock.publish(131 * CYCLES_PER_CLKN_TICK);
        assert_eq!(bt.tick().explicit_irqs, Some(vec![RWBLE_IRQ_SOURCE]));
        bt.write_u32(INTACK, INT_TIMER_HUS).unwrap();
        assert!(bt.matrix_irq_sources().is_empty());
        assert!(bt.tick().explicit_irqs.is_none());

        // Re-arming pushes the deadline out again — the advertising cadence.
        bt.write_u32(TIMER_HUS_TARGET, 280).unwrap();
        clock.publish(279 * CYCLES_PER_CLKN_TICK);
        assert!(bt.tick().explicit_irqs.is_none());
        clock.publish(280 * CYCLES_PER_CLKN_TICK);
        assert_eq!(bt.tick().explicit_irqs, Some(vec![RWBLE_IRQ_SOURCE]));
    }

    /// A comparator runs only while its `INTCNTL` enable is set. Silicon
    /// attests it: `+0x0E8` held a long-past `0x91` while CLKN was `0x462F`
    /// with `INTCNTL` bit 10 clear, and `INTRAWSTAT` bit 10 read 0. Modelling
    /// it the other way would raise a phantom interrupt out of a stale target.
    #[test]
    fn a_masked_comparator_does_not_latch() {
        let clock = CycleClock::default();
        let mut bt = advertising_part(&clock);
        assert_eq!(
            bt.read_u32(INTCNTL).unwrap() & INT_TIMER_HS,
            0,
            "the silicon enable word leaves the hs timer disarmed"
        );
        bt.write_u32(TIMER_HS_TARGET, 0x91).unwrap();
        clock.publish(0x462f * CYCLES_PER_CLKN_TICK);
        bt.tick();
        assert_eq!(
            bt.read_u32(INTRAWSTAT).unwrap() & INT_TIMER_HS,
            0,
            "a stale target behind a clear enable must not latch"
        );
        // Arm it (INTCNTL |= 0x400, as `r_rwip_timer_hs_set` does) and the same
        // stale target fires at once — a missed deadline, not a 23-hour wrap.
        bt.write_u32(INTCNTL, 0x0064_0b66 | INT_TIMER_HS).unwrap();
        assert_eq!(bt.tick().explicit_irqs, Some(vec![RWBLE_IRQ_SOURCE]));
        assert_eq!(
            bt.read_u32(INTRAWSTAT).unwrap() & INT_TIMER_HS,
            INT_TIMER_HS
        );
    }

    /// The 10 ms comparator counts in units of 32 CLKN ticks —
    /// `r_rwip_timer_10ms_set` writes the target to `+0x0E4` and keeps
    /// `rwip_env+8 = target << 5` (half-slots) alongside it.
    #[test]
    fn ten_ms_comparator_counts_in_32_clkn_units() {
        let clock = CycleClock::default();
        let mut bt = advertising_part(&clock);
        assert_ne!(bt.read_u32(INTCNTL).unwrap() & INT_TIMER_10MS, 0);
        bt.write_u32(TIMER_10MS_TARGET, 100).unwrap(); // 1 s = 3200 CLKN ticks
        clock.publish(3199 * CYCLES_PER_CLKN_TICK);
        assert!(bt.tick().explicit_irqs.is_none());
        clock.publish(3200 * CYCLES_PER_CLKN_TICK);
        assert_eq!(bt.tick().explicit_irqs, Some(vec![RWBLE_IRQ_SOURCE]));
        assert_eq!(
            bt.read_u32(INTSTAT).unwrap() & INT_TIMER_10MS,
            INT_TIMER_10MS
        );
    }

    /// The IRQ FIFO at `+0x2D8`. `sdk_cfg_priv_opts[69]` reads 1 on this
    /// silicon, so `r_rwble_isr` dispatches from here rather than from a raw
    /// `INTSTAT` read — and returns WITHOUT acking when `cnt == 0`, which
    /// would turn a raised level into an interrupt storm. Silicon read
    /// `+0x2D8 = 0x0020_003E` with `INTSTAT = 0x800`: cnt 1, rem 15,
    /// bitmap `0x800`.
    #[test]
    fn irq_fifo_matches_the_silicon_word() {
        let clock = CycleClock::default();
        let mut bt = advertising_part(&clock);
        assert_eq!(
            bt.read_u32(IRQ_FIFO).unwrap(),
            0x0000_001E,
            "empty FIFO: the exact word silicon reads while idle (cnt 0, rem 15)"
        );

        bt.write_u32(TIMER_HUS_TARGET, 130).unwrap();
        bt.write_u32(TIMER_HUS_FINE, 624).unwrap();
        clock.publish(130 * CYCLES_PER_CLKN_TICK);
        bt.tick();
        assert_eq!(
            bt.read_u32(IRQ_FIFO).unwrap(),
            0x0020_003E,
            "one queued hus interrupt must read back the exact silicon word"
        );

        // `ori a5,a5,1; sw` pops the head.
        bt.write_u32(IRQ_FIFO, 0x0020_003F).unwrap();
        assert_eq!(bt.read_u32(IRQ_FIFO).unwrap() >> 5 & 31, 0, "cnt back to 0");
        assert_eq!(bt.read_u32(IRQ_FIFO).unwrap() >> 10, 0, "no head bitmap");
        // Popping the FIFO is NOT the ack: the raw latch is separate, and the
        // ISR clears it through `+0x018`.
        assert_eq!(bt.read_u32(INTRAWSTAT).unwrap(), INT_TIMER_HUS);
    }

    /// One entry per rising edge, capped at the 16-deep FIFO — never a
    /// re-queue while the same bit is still latched, which would let one
    /// unacked interrupt flood the queue.
    #[test]
    fn irq_fifo_queues_one_entry_per_rising_edge() {
        let clock = CycleClock::default();
        let mut bt = advertising_part(&clock);
        bt.write_u32(TIMER_HUS_TARGET, 10).unwrap();
        bt.write_u32(TIMER_HUS_FINE, 624).unwrap();
        for n in 10..40u64 {
            clock.publish(n * CYCLES_PER_CLKN_TICK);
            bt.tick();
        }
        assert_eq!(bt.irq_fifo.borrow().len(), 1, "still one unacked interrupt");
    }

    /// The walk must not run while there is nothing scheduled, and must run
    /// the moment a comparator is armed or the line is up.
    #[test]
    fn walk_membership_follows_the_comparators() {
        let clock = CycleClock::default();
        let mut bt = Esp32c3Bt::new();
        bt.attach_cycle_clock(clock.clone());
        clock.publish(0);
        // Walk membership is only claimed in legacy mode; under the scheduler
        // the comparators ride events instead (and the C3 walk-pinner ledger
        // requires that).
        assert_eq!(bt.needs_legacy_walk(), !bt.uses_scheduler());
        assert!(bt.legacy_tick_dynamic());
        assert!(!bt.legacy_tick_active(), "gated block has nothing to tick");
        bt.write_u32(RWBLECNTL, 0x0010_070f).unwrap();
        assert!(!bt.legacy_tick_active(), "un-gated but nothing armed");
        bt.write_u32(INTCNTL, INT_TIMER_HUS).unwrap();
        assert!(
            !bt.legacy_tick_active(),
            "an enable over an unprogrammed target is not an armed comparator"
        );
        bt.write_u32(TIMER_HUS_TARGET, 130).unwrap();
        assert!(bt.legacy_tick_active(), "hus comparator armed");
    }

    /// Scheduler mode: the hus comparator must arrive as a scheduled event at
    /// its exact cycle, with no per-cycle walk and no firmware poll — the same
    /// contract `ledc` holds for `LSTIMERx_OVF`. This is what keeps the model
    /// off the C3 walk-pinner ledger.
    #[cfg(feature = "event-scheduler")]
    #[test]
    fn scheduled_event_delivers_the_comparator_without_a_walk() {
        use crate::sched::EventScheduler;

        let clock = CycleClock::default();
        let mut bt = advertising_part(&clock);
        assert!(bt.uses_scheduler(), "a clocked model is scheduler-driven");
        assert!(!bt.needs_legacy_walk(), "and must not pin the walk");

        // Arm 130 CLKN ticks out, exactly as `r_rwip_timer_hus_set` does.
        bt.write_u32(TIMER_HUS_TARGET, 130).unwrap();
        bt.write_u32(TIMER_HUS_FINE, 624).unwrap();
        bt.write_u32(INTACK, INT_TIMER_HUS).unwrap();
        let events = bt.take_scheduled_events();
        assert_eq!(events.len(), 1, "one in-flight comparator event");
        let (delay, token) = events[0];
        assert_eq!(
            delay,
            130 * CYCLES_PER_CLKN_TICK - 1,
            "the bus adds the +1 anchor offset back"
        );

        let mut sched = EventScheduler::new();
        let mut bus = crate::bus::SystemBus::new();
        sched.advance_to(130 * CYCLES_PER_CLKN_TICK);
        clock.publish(130 * CYCLES_PER_CLKN_TICK);
        let res = bt.on_event(token, &mut sched, &mut bus);
        assert_eq!(bt.read_u32(INTSTAT).unwrap(), INT_TIMER_HUS);
        assert_eq!(bt.matrix_irq_sources(), vec![RWBLE_IRQ_SOURCE]);
        assert!(
            res.reschedule_delay.is_none(),
            "nothing else armed, so the chain stops until firmware re-arms"
        );

        // A stale generation must not fire anything. The clock stays behind
        // the new deadline so the lazy read-path latch cannot mask the check.
        bt.write_u32(INTACK, INT_TIMER_HUS).unwrap();
        bt.write_u32(TIMER_HUS_TARGET, 300).unwrap();
        let fresh = bt.take_scheduled_events()[0].1;
        assert_ne!(token, fresh, "re-arming stamps a fresh generation");
        sched.advance_to(300 * CYCLES_PER_CLKN_TICK);
        bt.on_event(token, &mut sched, &mut bus);
        assert_eq!(bt.read_u32(INTSTAT).unwrap(), 0, "stale token is inert");
        bt.on_event(fresh, &mut sched, &mut bus);
        assert_eq!(bt.read_u32(INTSTAT).unwrap(), INT_TIMER_HUS);
    }

    /// CLKN must be monotonic — the event scheduler re-reads it to decide
    /// whether its deadline already slipped.
    #[test]
    fn clkn_is_monotonic() {
        let mut bt = Esp32c3Bt::new();
        let clock = CycleClock::default();
        bt.attach_cycle_clock(clock.clone());
        clock.publish(0);
        bt.write_u32(0x000, 1).unwrap();
        let mut last = 0;
        for n in 0..5_000u64 {
            clock.publish(n * 9_973);
            let now = bt.read_u32(CLKN).unwrap();
            assert!(now >= last, "CLKN went backwards: {last} -> {now}");
            last = now;
        }
        assert!(last > 0, "CLKN never advanced");
    }
}
