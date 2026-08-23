// LabWired - Firmware Simulation Platform
// SPDX-License-Identifier: MIT

//! EFR32MG26 TIMER compare semantics, replayed from a **BRD2709A die**.
//!
//! # What the die was asked
//!
//! A physical BRD2709A was driven over SWD with the sequence this file
//! replays, register for register and in the same order: TIMER0 at
//! `0x40048000`, `CFG = 0x0FFC0040` (`PRESC = 1023`, `DEBUGRUN`),
//! `TOP = 0xFFFF`, CC0 in OUTPUTCOMPARE with `CC0_OC = 0x8000`, `CNT`
//! pre-loaded, `IF` cleared, then `CMD.START`. One counter tick is ~53 µs and
//! a full period is ~3.5 s, so the 250 ms read lands well inside one pass.
//!
//! | case                       | start `CNT` | die `IF` @ 250 ms |
//! |----------------------------|-------------|-------------------|
//! | B, the counter ARRIVES     | `0x7FFF`    | `0x10` (CC0)      |
//! | A, the counter is PLACED   | `0x8000`    | `0x10` (CC0)      |
//! | A, after the wrap (3.75 s) | `0x8000`    | `0x11` (CC0 + OF) |
//!
//! # What it proves
//!
//! Case A is the row an arrival-edge compare cannot produce. `CNT` was written
//! equal to `OC` and the counter then climbed `0x8000 → 0x9268` — it never
//! returns to `0x8000` before the wrap, so the flag is not a second pass, and
//! `IF` read back `0x00000000` after setup and before `CMD.START`, so it is not
//! stale either. **The EFR32 compare is a level match on `CNT == OC`, sampled
//! by the counter clock**: a value the counter is merely SITTING on when it
//! starts running matches, once, on the first counter step.
//!
//! The other half of the same measurement is what keeps that from being a
//! standing rule: `IF` stayed 0 while `CNT == OC` and the counter was stopped,
//! so the match is gated on the counter actually being clocked. The model
//! spells this as a one-shot — see `Efr32s2Timer::level_pending`.
//!
//! ⚠️ Series-2 register discipline, and it cost two die runs to learn: `CFG` is
//! a config register and must be written while `EN = 0`; `TOP`, `CNT` and
//! `CC_OC` are runtime registers and must be written AFTER `EN = 1`, or they
//! read back 0 and the run proves something about a timer that was never
//! configured. This file writes them in the order the die was given.

use labwired_config::{ChipDescriptor, SystemManifest};
use labwired_core::bus::SystemBus;
use labwired_core::Bus;
use std::path::PathBuf;

const TIMER0: u64 = 0x4004_8000;
const CFG: u64 = TIMER0 + 0x04;
const CMD: u64 = TIMER0 + 0x0C;
const IF: u64 = TIMER0 + 0x14;
const TOP: u64 = TIMER0 + 0x1C;
const CNT: u64 = TIMER0 + 0x24;
const EN: u64 = TIMER0 + 0x30;
const CC0_CFG: u64 = TIMER0 + 0x60;
const CC0_OC: u64 = TIMER0 + 0x68;

/// `CMU_CLKEN0`, and the value the die was given: TIMER0's clock among others.
/// Without it a Series-2 peripheral is gated off and the whole run is silent.
const CMU_CLKEN0: u64 = 0x4000_8064;
const CLKEN0_VALUE: u32 = 0x0400_4630;

const IF_OF: u32 = 1 << 0;
const IF_CC0: u32 = 1 << 4;

/// `PRESC = 1023`: 1024 timer clocks per counter tick.
const CYCLES_PER_TICK: u64 = 1024;

fn root(rel: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../..");
    p.push(rel);
    p
}

fn brd2709a_bus() -> SystemBus {
    let system_path = root("configs/systems/brd2709a.yaml");
    let mut manifest = SystemManifest::from_file(&system_path).expect("load brd2709a manifest");
    // Derive walk-deletion from the models, never from the yaml escape hatch.
    manifest.walk_deleted = None;
    let chip_path = system_path.parent().unwrap().join(&manifest.chip);
    let chip = ChipDescriptor::from_file(&chip_path).expect("load efr32mg26 chip");
    SystemBus::from_config(&chip, &manifest).expect("build brd2709a bus")
}

/// Advance every peripheral on the bus by exactly `cycles`.
///
/// ⚠️ `_forced`, not the plain tick. This harness has no `Machine`, so nothing
/// drains the event scheduler — and under `--features event-scheduler` the
/// TIMER is scheduler-driven, which means the ordinary walk deliberately skips
/// it and this would advance NOTHING. `tick_peripherals_fully_forced` is the
/// documented boundary for a frozen-CPU harness, and it makes this file assert
/// the same thing under both feature sets. The scheduler lane's own arming is
/// pinned separately, by `efr32mg26_walk_differential` and by the in-module
/// `a_pending_level_match_arms_the_very_next_step`.
fn advance_ticks(bus: &mut SystemBus, ticks: u64) {
    let mut left = ticks * CYCLES_PER_TICK;
    while left > 0 {
        let step = left.min(1 << 20);
        bus.config.peripheral_tick_interval = step as u32;
        bus.tick_peripherals_fully_forced();
        left -= step;
    }
}

/// The die's setup, in the die's order. Returns the bus with the counter
/// configured and stopped.
fn die_setup(start_cnt: u32) -> SystemBus {
    let mut bus = brd2709a_bus();
    bus.write_u32(CMU_CLKEN0, CLKEN0_VALUE).unwrap();
    bus.write_u32(EN, 0).unwrap();
    bus.write_u32(CFG, 0x0FFC_0040).unwrap();
    bus.write_u32(CC0_CFG, 0x0000_0002).unwrap(); // OUTPUTCOMPARE
    bus.write_u32(EN, 1).unwrap();
    bus.write_u32(TOP, 0x0000_FFFF).unwrap();
    bus.write_u32(CC0_OC, 0x0000_8000).unwrap();
    bus.write_u32(CNT, start_cnt).unwrap();
    bus.write_u32(IF, 0xFFFF_FFFF).unwrap();
    bus
}

/// Case B, the control: the counter ARRIVES at the compare value. An arrival
/// edge and a level match both produce this one.
#[test]
fn efr32mg26_timer0_flags_a_compare_the_counter_arrives_at() {
    let mut bus = die_setup(0x0000_7FFF);
    assert_eq!(
        bus.read_u32(IF).unwrap(),
        0,
        "the die read IF = 0 after setup and before START"
    );

    bus.write_u32(CMD, 1).unwrap(); // START
    advance_ticks(&mut bus, 4712); // the die's CNT position at the 250 ms read

    assert_eq!(bus.read_u32(CNT).unwrap(), 0x9267);
    assert_eq!(
        bus.read_u32(IF).unwrap(),
        IF_CC0,
        "CC0 set, no overflow yet — the die reads 0x10"
    );
}

/// ⚠️ Case A: the counter is WRITTEN onto its compare value and started. The
/// die flags CC0; an arrival-edge model stays silent for a full 3.5 s period.
/// This is the row the model got wrong.
#[test]
fn efr32mg26_timer0_flags_a_compare_the_counter_was_started_on() {
    let mut bus = die_setup(0x0000_8000);
    assert_eq!(
        bus.read_u32(IF).unwrap(),
        0,
        "the die read IF = 0 here too, with CNT already equal to OC: the match \
         is gated on the counter being CLOCKED, not on the register values \
         alone"
    );

    bus.write_u32(CMD, 1).unwrap(); // START
    advance_ticks(&mut bus, 4712);

    assert_eq!(
        bus.read_u32(CNT).unwrap(),
        0x9268,
        "the counter climbed away from 0x8000 and cannot have re-reached it \
         inside one period, so the flag below is not a second pass"
    );
    assert_eq!(
        bus.read_u32(IF).unwrap(),
        IF_CC0,
        "the die reads 0x10: a level match on CNT == OC, with no arrival"
    );
}

/// The third die row: the same pre-loaded run carried past the wrap picks up
/// the overflow and keeps the compare.
#[test]
fn efr32mg26_timer0_adds_the_overflow_once_the_pre_loaded_run_wraps() {
    let mut bus = die_setup(0x0000_8000);
    bus.write_u32(CMD, 1).unwrap();
    advance_ticks(&mut bus, 70_000); // ~3.75 s, past the ~3.5 s period

    assert_eq!(
        bus.read_u32(IF).unwrap(),
        IF_CC0 | IF_OF,
        "the die reads 0x11"
    );
}

/// ⚠️ The level match must be a ONE-SHOT on the value that was placed.
///
/// A standing "also compare the value you start from" rule reproduces the die
/// rows above and still breaks the part: under the walk every advance starts
/// from the value the previous one landed on, so every compare re-latches on
/// the next tick and firmware that acknowledges in its handler takes two
/// interrupts per match. Here the handler clears `IF` while `CNT` still sits on
/// the compare value — the flag must stay clear until the next period.
#[test]
fn efr32mg26_timer0_does_not_re_latch_a_compare_the_counter_is_leaving() {
    let mut bus = die_setup(0x0000_7FFF);
    bus.write_u32(CMD, 1).unwrap();

    advance_ticks(&mut bus, 1); // arrives at 0x8000
    assert_eq!(bus.read_u32(CNT).unwrap(), 0x8000);
    assert_eq!(bus.read_u32(IF).unwrap(), IF_CC0);

    bus.write_u32(IF, 0xFFFF_FFFF).unwrap(); // the handler acknowledges
    advance_ticks(&mut bus, 1);
    assert_eq!(
        bus.read_u32(IF).unwrap(),
        0,
        "the counter is LEAVING 0x8000; re-latching here would double every \
         compare interrupt this part raises"
    );

    // ...and the next real match, a full period on, still lands.
    advance_ticks(&mut bus, 0x1_0000 - 1);
    assert_eq!(bus.read_u32(CNT).unwrap(), 0x8000);
    assert_eq!(bus.read_u32(IF).unwrap(), IF_CC0 | IF_OF);
}
