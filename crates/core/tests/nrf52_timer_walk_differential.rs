// LabWired - Firmware Simulation Platform
// SPDX-License-Identifier: MIT

//! Walk≡scheduler fidelity gates for nRF52 TIMER / RTC / RADIO at tick 512.
//!
//! Nrf52Timer / Nrf52Rtc have no `force_legacy_walk` knob (scheduler mode is
//! clock-presence). Dual lanes therefore use the production walk-free
//! nRF52840 DK bus with:
//!
//! - **Lane A** — `peripheral_tick_interval = 1` (every-cycle drain + bus_tick)
//! - **Lane B** — `peripheral_tick_interval = RECOMMENDED_TICK_INTERVAL` (512)
//!
//! Both ride the event scheduler under Machine + walk-free; completion is
//! observed with single-cycle advance batches so absolute cycle identity is
//! measurable (not quantised to a 512-batch boundary).
//!
//! Surfaces:
//! 1. TIMER0 COMPARE[0] — program CC/START/INTEN, assert same fire cycle
//!    (within 1) and EVENTS_COMPARE[0] on both lanes.
//! 2. RTC0 COMPARE[0] — EVTEN+INTEN compare path (not COUNTER poll-only).
//! 3. RADIO TXEN → START → END — delay-0 / short countdown identity (bit-rate
//!    full matrix remains interim on the scoreboard).
//!
//! Requires `--features event-scheduler`.

#![cfg(feature = "event-scheduler")]

use labwired_config::{ChipDescriptor, SystemManifest};
use labwired_core::bus::{SystemBus, RECOMMENDED_TICK_INTERVAL};
use labwired_core::snapshot::{ArmCpuSnapshot, CpuSnapshot};
use labwired_core::system::cortex_m::configure_cortex_m;
use labwired_core::{
    AdvanceRequest, BreakpointPolicy, Bus, Cpu, Machine, SimResult, SimulationConfig,
    SimulationObserver,
};
use std::path::PathBuf;
use std::sync::Arc;

fn root(rel: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../..");
    p.push(rel);
    p
}

/// Minimal cycle-advancing CPU: retires one cycle per step so `Machine`
/// can drain the event scheduler without needing real Thumb firmware.
#[derive(Debug, Default)]
struct CycleCpu {
    pc: u32,
    steps: u32,
}

impl Cpu for CycleCpu {
    fn reset(&mut self, _bus: &mut dyn Bus) -> SimResult<()> {
        self.pc = 0;
        self.steps = 0;
        Ok(())
    }

    fn step(
        &mut self,
        _bus: &mut dyn Bus,
        _observers: &[Arc<dyn SimulationObserver>],
        _config: &SimulationConfig,
    ) -> SimResult<()> {
        self.steps = self.steps.wrapping_add(1);
        self.pc = self.pc.wrapping_add(2);
        Ok(())
    }

    fn set_pc(&mut self, val: u32) {
        self.pc = val;
    }

    fn get_pc(&self) -> u32 {
        self.pc
    }

    fn set_sp(&mut self, _val: u32) {}

    fn set_exception_pending(&mut self, _exception_num: u32) {}

    fn get_register(&self, id: u8) -> u32 {
        match id {
            0 => self.steps,
            15 => self.pc,
            _ => 0,
        }
    }

    fn set_register(&mut self, id: u8, val: u32) {
        match id {
            0 => self.steps = val,
            15 => self.pc = val,
            _ => {}
        }
    }

    fn snapshot(&self) -> CpuSnapshot {
        let mut registers = vec![0; 16];
        registers[0] = self.steps;
        registers[15] = self.pc;
        CpuSnapshot::Arm(ArmCpuSnapshot {
            registers,
            pc: self.pc,
            xpsr: 0,
            primask: false,
            pending_exceptions: 0,
            pending_exceptions_hi: Vec::new(),
            vtor: 0,
        })
    }

    fn apply_snapshot(&mut self, snapshot: &CpuSnapshot) {
        if let CpuSnapshot::Arm(s) = snapshot {
            self.steps = s.registers.first().copied().unwrap_or(0);
            self.pc = s.pc;
        }
    }

    fn get_register_names(&self) -> Vec<String> {
        (0..=12)
            .map(|id| format!("R{id}"))
            .chain(["SP", "LR", "PC"].into_iter().map(String::from))
            .collect()
    }

    fn index_of_register(&self, name: &str) -> Option<u8> {
        if name.eq_ignore_ascii_case("PC") {
            return Some(15);
        }
        let id = name
            .strip_prefix('R')
            .or_else(|| name.strip_prefix('r'))?
            .parse::<u8>()
            .ok()?;
        (id <= 12).then_some(id)
    }
}

fn bus_nrf52840_walk_free() -> SystemBus {
    let chip = ChipDescriptor::from_file(&root("configs/chips/nrf52840.yaml"))
        .expect("load nrf52840 chip");
    let system_path = root("configs/systems/nrf52840-dk.yaml");
    let mut manifest =
        SystemManifest::from_file(&system_path).expect("load nrf52840-dk system");
    let anchored = system_path
        .parent()
        .expect("system parent")
        .join(&manifest.chip);
    manifest.chip = anchored.to_string_lossy().into_owned();
    // Auto-derive walk deletion (never honor a hand walk_deleted hatch).
    manifest.walk_deleted = None;
    let mut bus = SystemBus::from_config(&chip, &manifest).expect("build nrf52840 bus");
    let _ = configure_cortex_m(&mut bus);
    bus
}

fn machine_at_interval(interval: u32) -> Machine<CycleCpu> {
    let bus = bus_nrf52840_walk_free();
    assert!(
        bus.legacy_walk_disabled,
        "precondition: walk-free auto-derive failed"
    );
    assert_eq!(
        bus.max_safe_tick_interval(),
        RECOMMENDED_TICK_INTERVAL,
        "precondition: max_safe must be {RECOMMENDED_TICK_INTERVAL}"
    );
    let mut machine = Machine::new(CycleCpu::default(), bus);
    machine.config.peripheral_tick_interval = interval;
    machine.bus.config.peripheral_tick_interval = interval;
    machine
}

/// Advance ≤ `max_cycles` one cycle at a time; return total_cycles when
/// `done` returns true, or None if the budget is exhausted.
fn advance_until<F>(
    machine: &mut Machine<CycleCpu>,
    max_cycles: u64,
    mut done: F,
) -> Option<u64>
where
    F: FnMut(&Machine<CycleCpu>) -> bool,
{
    while machine.total_cycles < max_cycles {
        machine
            .advance(
                AdvanceRequest::run(Some(1)).with_breakpoints(BreakpointPolicy::Ignore),
            )
            .expect("Machine::advance");
        if done(machine) {
            return Some(machine.total_cycles);
        }
    }
    None
}

fn assert_cycle_identity(at_a: u64, at_b: u64, what: &str) {
    let delta = at_a.abs_diff(at_b);
    assert!(
        delta <= 1,
        "{what}: completion cycle must agree within 1 \
         (interval=1 at={at_a}, interval={RECOMMENDED_TICK_INTERVAL} at={at_b}, delta={delta})"
    );
}

// ── TIMER0 COMPARE ──────────────────────────────────────────────────────────

const TIMER0: u64 = 0x4000_8000;
const TIMER_TASKS_START: u64 = TIMER0;
const TIMER_TASKS_CLEAR: u64 = TIMER0 + 0x00C;
const TIMER_EVENTS_COMPARE0: u64 = TIMER0 + 0x140;
const TIMER_INTENSET: u64 = TIMER0 + 0x304;
const TIMER_BITMODE: u64 = TIMER0 + 0x508;
const TIMER_PRESCALER: u64 = TIMER0 + 0x510;
const TIMER_CC0: u64 = TIMER0 + 0x540;

fn arm_timer0_compare(machine: &mut Machine<CycleCpu>, cc: u32) {
    machine.bus.write_u32(TIMER_BITMODE, 3).unwrap(); // 32-bit
    machine.bus.write_u32(TIMER_PRESCALER, 0).unwrap();
    machine.bus.write_u32(TIMER_CC0, cc).unwrap();
    machine.bus.write_u32(TIMER_INTENSET, 1 << 16).unwrap(); // COMPARE[0]
    machine.bus.write_u32(TIMER_EVENTS_COMPARE0, 0).unwrap();
    machine.bus.write_u32(TIMER_TASKS_CLEAR, 1).unwrap();
    machine.bus.write_u32(TIMER_TASKS_START, 1).unwrap();
}

fn timer_compare_done(m: &Machine<CycleCpu>) -> bool {
    m.bus.read_u32(TIMER_EVENTS_COMPARE0).unwrap_or(0) != 0
}

/// TIMER0 COMPARE[0]: walk-free interval 1 vs interval 512 — same fire cycle
/// (within 1) and same EVENTS_COMPARE[0] latch.
#[test]
fn timer0_compare_walk1_vs_sched512_cycle_identity() {
    const CC: u32 = 8;
    // CC=8 at PRESCALER=0 → match after 8 base ticks; headroom for arm lag.
    const BUDGET: u64 = 64;

    let mut lane_a = machine_at_interval(1);
    arm_timer0_compare(&mut lane_a, CC);
    let at_a = advance_until(&mut lane_a, BUDGET, timer_compare_done)
        .expect("lane A (interval=1) must fire TIMER0 COMPARE[0]");

    let mut lane_b = machine_at_interval(RECOMMENDED_TICK_INTERVAL);
    arm_timer0_compare(&mut lane_b, CC);
    let at_b = advance_until(&mut lane_b, BUDGET, timer_compare_done).expect(
        "lane B (interval=512) must fire TIMER0 COMPARE[0] via scheduler",
    );

    assert!(
        timer_compare_done(&lane_a) && timer_compare_done(&lane_b),
        "both lanes must latch EVENTS_COMPARE[0]"
    );
    assert_cycle_identity(at_a, at_b, "TIMER0 COMPARE[0]");
}

// ── RTC0 COMPARE (EVTEN + INTEN path) ───────────────────────────────────────

const RTC0: u64 = 0x4000_B000;
const RTC_TASKS_START: u64 = RTC0;
const RTC_TASKS_CLEAR: u64 = RTC0 + 0x008;
const RTC_EVENTS_COMPARE0: u64 = RTC0 + 0x140;
const RTC_INTENSET: u64 = RTC0 + 0x304;
const RTC_EVTENSET: u64 = RTC0 + 0x344;
const RTC_PRESCALER: u64 = RTC0 + 0x508;
const RTC_CC0: u64 = RTC0 + 0x540;

fn arm_rtc0_compare(machine: &mut Machine<CycleCpu>, cc: u32) {
    // PRESCALER must be written while stopped.
    machine.bus.write_u32(RTC_PRESCALER, 0).unwrap();
    machine.bus.write_u32(RTC_CC0, cc).unwrap();
    // EVTEN required for EVENTS_COMPARE latch; INTEN for IRQ surface.
    machine.bus.write_u32(RTC_EVTENSET, 1 << 16).unwrap();
    machine.bus.write_u32(RTC_INTENSET, 1 << 16).unwrap();
    machine.bus.write_u32(RTC_EVENTS_COMPARE0, 0).unwrap();
    machine.bus.write_u32(RTC_TASKS_CLEAR, 1).unwrap();
    machine.bus.write_u32(RTC_TASKS_START, 1).unwrap();
}

fn rtc_compare_done(m: &Machine<CycleCpu>) -> bool {
    m.bus.read_u32(RTC_EVENTS_COMPARE0).unwrap_or(0) != 0
}

/// RTC0 COMPARE[0] (EVTEN+INTEN): interval 1 vs 512 — same fire cycle within 1.
///
/// COUNTER poll-only is deliberately not certified here (see scoreboard
/// interim); this gate covers the compare / IRQ-driven shape only.
///
/// Real LFCLK ratio (~1953 CPU cycles per RTC tick) with CC[0]=2 needs
/// ~3906 cycles; budget keeps clear of batch-edge ambiguity.
#[test]
fn rtc0_compare_walk1_vs_sched512_cycle_identity() {
    const CC: u32 = 2;
    // 2 × ~1953 ≈ 3906; generous headroom for scheduler arm / LFCLK fraction.
    const BUDGET: u64 = 12_000;

    let mut lane_a = machine_at_interval(1);
    arm_rtc0_compare(&mut lane_a, CC);
    let at_a = advance_until(&mut lane_a, BUDGET, rtc_compare_done)
        .expect("lane A (interval=1) must fire RTC0 COMPARE[0]");

    let mut lane_b = machine_at_interval(RECOMMENDED_TICK_INTERVAL);
    arm_rtc0_compare(&mut lane_b, CC);
    let at_b = advance_until(&mut lane_b, BUDGET, rtc_compare_done)
        .expect("lane B (interval=512) must fire RTC0 COMPARE[0] via scheduler");

    assert!(
        rtc_compare_done(&lane_a) && rtc_compare_done(&lane_b),
        "both lanes must latch EVENTS_COMPARE[0] (EVTEN compare path)"
    );
    assert_cycle_identity(at_a, at_b, "RTC0 COMPARE[0]");
}

// ── RADIO TXEN → START → END (minimal) ──────────────────────────────────────

const RADIO: u64 = 0x4000_1000;
const RADIO_TASKS_TXEN: u64 = RADIO;
const RADIO_TASKS_START: u64 = RADIO + 0x008;
const RADIO_EVENTS_READY: u64 = RADIO + 0x100;
const RADIO_EVENTS_END: u64 = RADIO + 0x10C;
const RADIO_SHORTS: u64 = RADIO + 0x200;
const RADIO_PACKETPTR: u64 = RADIO + 0x504;
const RADIO_FREQUENCY: u64 = RADIO + 0x508;
const RADIO_MODE: u64 = RADIO + 0x510;
const RADIO_PCNF0: u64 = RADIO + 0x514;
const RADIO_PCNF1: u64 = RADIO + 0x518;
const RADIO_BASE0: u64 = RADIO + 0x51C;
const RADIO_PREFIX0: u64 = RADIO + 0x524;
// SHORTS bit 0 = READY_START (PS table 224).
const SHORT_READY_START: u32 = 1 << 0;

fn plant_radio_tx_buf(bus: &mut SystemBus, base: u64) {
    // Minimal S0 + LENGTH=1 + 1 payload byte (BLE-like layout).
    bus.write_u8(base, 0x40).unwrap(); // S0
    bus.write_u8(base + 1, 1).unwrap(); // LENGTH
    bus.write_u8(base + 2, 0xA5).unwrap(); // payload
}

fn arm_radio_tx(machine: &mut Machine<CycleCpu>, buf: u64) {
    plant_radio_tx_buf(&mut machine.bus, buf);
    machine.bus.write_u32(RADIO_FREQUENCY, 0x4E).unwrap(); // BLE adv ch 37
    machine.bus.write_u32(RADIO_MODE, 0x3).unwrap(); // BLE_1Mbit
    machine.bus.write_u32(RADIO_PCNF0, 0x0001_0008).unwrap();
    machine.bus.write_u32(RADIO_PCNF1, 0x0003_00FF).unwrap();
    machine.bus.write_u32(RADIO_BASE0, 0xCAFE_BABE).unwrap();
    machine.bus.write_u32(RADIO_PREFIX0, 0xDEAD).unwrap();
    machine.bus.write_u32(RADIO_PACKETPTR, buf as u32).unwrap();
    machine.bus.write_u32(RADIO_SHORTS, SHORT_READY_START).unwrap();
    machine.bus.write_u32(RADIO_EVENTS_READY, 0).unwrap();
    machine.bus.write_u32(RADIO_EVENTS_END, 0).unwrap();
    machine.bus.write_u32(RADIO_TASKS_TXEN, 1).unwrap();
}

fn radio_end_done(m: &Machine<CycleCpu>) -> bool {
    m.bus.read_u32(RADIO_EVENTS_END).unwrap_or(0) != 0
}

/// RADIO TX start chain: TXEN + READY_START short → EVENTS_END.
/// Interval 1 vs 512 must raise END and agree on completion cycle within 1
/// for the default/short countdown path. Full bit-rate matrix is interim.
#[test]
fn radio_tx_end_walk1_vs_sched512_cycle_identity() {
    // Delay-0 READY + DMA + short bit-rate countdown → END well under 64.
    const BUDGET: u64 = 128;
    let buf = 0x2000_2000u64;

    let mut lane_a = machine_at_interval(1);
    arm_radio_tx(&mut lane_a, buf);
    // Explicit START is redundant with SHORTS READY_START but harmless if
    // READY has not yet fired; TXEN arms the scheduler chain.
    let _ = lane_a.bus.write_u32(RADIO_TASKS_START, 1);
    let at_a = advance_until(&mut lane_a, BUDGET, radio_end_done)
        .expect("lane A (interval=1) must raise RADIO EVENTS_END");

    let mut lane_b = machine_at_interval(RECOMMENDED_TICK_INTERVAL);
    arm_radio_tx(&mut lane_b, buf);
    let _ = lane_b.bus.write_u32(RADIO_TASKS_START, 1);
    let at_b = advance_until(&mut lane_b, BUDGET, radio_end_done)
        .expect("lane B (interval=512) must raise RADIO EVENTS_END via scheduler");

    assert!(
        radio_end_done(&lane_a) && radio_end_done(&lane_b),
        "both lanes must latch EVENTS_END"
    );
    assert_cycle_identity(at_a, at_b, "RADIO TX→END");
}
