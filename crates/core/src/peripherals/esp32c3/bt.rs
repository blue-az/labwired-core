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
//!   *What the top bit actually means is NOT settled, and the model does not
//!   pretend otherwise.* Across the trace the written low bits climb
//!   (`0x8000_0000`, `0x8000_5fd1`, `0x8000_c0a3`, `0x8000_e8f7`, …
//!   `0x8001_1658`) at a rate consistent with the clock over the wall time the
//!   traced run took, which reads as a *sample/latch* idiom (read the counter,
//!   write it back with the control bit to latch a coherent
//!   `BASETIMECNT`/`FINETIMECNT` pair) rather than as a next-event comparator.
//!   Both readings demand the same modelled behaviour — read returns the live
//!   clock, write does not clobber it — so the ambiguity is recorded, not
//!   resolved by invention. The written value is kept in
//!   [`Esp32c3Bt::armed_event_target`] for whoever settles it.
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
//! * **The interrupt path.** Silicon capture 2026-08-02 of the C3 interrupt
//!   matrix after init: `RWBLE_IRQ_MAP` (`0x600C_2020`) = 5 and `BT_BB_INT_MAP`
//!   (`0x600C_2014`) = 8, i.e. the firmware routes matrix source 8 (RWBLE) to
//!   CPU line 5 and source 5 (BT_BB) to CPU line 8. The write trace shows the
//!   ISR-side registers (`+0x018` written `0xffffffff` / `0x200` / `0x800`,
//!   mirrored at `+0x38C`, with `+0x010`/`+0x014` reading back matching bits —
//!   so `+0x014` looks like a raw status and `+0x018` a W1C clear). That is
//!   enough to *name* the path but not enough to know which bit each event
//!   raises, so this model does not yet fire [`Self::matrix_irq_sources_into`].
//!   Radio events therefore do not arrive; the comparator armed at `+0x01C` is
//!   recorded ([`Esp32c3Bt::event_target`]) and left for that follow-up.
//! * **The radio itself.** As with the WiFi MAC, there is no RF here. This is
//!   an air-gapped behavioral endpoint, not a faithful BLE PHY.

use crate::{CycleClock, Peripheral, SimResult};

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

/// Top bit of a `CLKN` write — the control/sample bit. Every traced write had
/// it set.
const CLKN_TARGET_ARM: u32 = 0x8000_0000;

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
    /// Last comparator target written to `CLKN` (low 31 bits), and whether the
    /// arm bit was set. Kept out of `regs` because a `CLKN` read must return
    /// the running clock, not this. Recorded but not yet acted on — see the
    /// interrupt note in the module docs.
    event_target: u32,
    event_armed: bool,
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
        ((self.elapsed_cycles() / CYCLES_PER_CLKN_TICK) & 0x7FFF_FFFF) as u32
    }

    /// Sub-CLKN fine counter: half-µs ticks, wrapping once per CLKN tick.
    #[inline]
    fn clkn_fine(&self) -> u32 {
        ((self.elapsed_cycles() / CYCLES_PER_FINE_TICK) % FINE_TICKS_PER_CLKN) as u32
    }

    /// The comparator target last armed via a `CLKN` write, if armed. Exposed
    /// for the interrupt follow-up and for tests.
    pub fn armed_event_target(&self) -> Option<u32> {
        self.event_armed.then_some(self.event_target)
    }
}

impl Peripheral for Esp32c3Bt {
    /// Inert walk: the timebase is derived on read from the bus cycle clock,
    /// and no interrupt level is exported yet, so there is nothing for the
    /// per-cycle walk to do. (See the interrupt note in the module docs — when
    /// the comparator learns to fire, this becomes a level export like the
    /// WiFi MAC's, not a tick.)
    fn needs_legacy_walk(&self) -> bool {
        false
    }

    fn legacy_tick_active(&self) -> bool {
        false
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
            _ => *self.regs.get((offset / 4) as usize).unwrap_or(&0),
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
            (0x000u64, 0x0010_070fu32), // control word, final post-init value
            (0x204, 0x0002_9725),       // ROM patch/veneer table entry 0
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

        // Fine counter: half-µs ticks, wrapping once per CLKN tick.
        clock.publish(1_000 + CYCLES_PER_FINE_TICK * 566);
        assert_eq!(bt.read_u32(CLKN_FINE).unwrap(), 566); // max value seen on silicon
        assert_eq!(bt.read_u32(CLKN).unwrap(), 0, "still inside the first tick");
        clock.publish(1_000 + CYCLES_PER_CLKN_TICK);
        assert_eq!(bt.read_u32(CLKN_FINE).unwrap(), 0, "wraps with CLKN");
        assert_eq!(bt.read_u32(CLKN).unwrap(), 1);

        // Never leaves the range silicon showed.
        for n in 0..2_000u64 {
            clock.publish(1_000 + n * 137);
            assert!(bt.read_u32(CLKN_FINE).unwrap() < FINE_TICKS_PER_CLKN as u32);
        }
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
