// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! [`JitHost`] adapter for a `Machine<RiscV>`.
//!
//! The universal dispatch loop drives the guest **entirely** through the
//! [`JitHost`] trait, never touching a concrete `Cpu`/`Bus`. This module is
//! the RISC-V binding: it wraps `&mut Machine<RiscV>` and implements every
//! hook — interpret one instruction, read the current PC, resume at a PC,
//! hand out a flash code view, report the safety gate, and flatten
//! architectural state into a [`StateVec`] for the differential harness.
//!
//! ## Two places the framework API did not match the RISC-V machine
//!
//! * **`code_view` is NOT backed by `Bus::fetch_slice`.** `SystemBus`'s
//!   `fetch_slice` only serves the Xtensa `RamPeripheral` (it downcasts to
//!   it and returns `None` otherwise), so it yields nothing for a RISC-V
//!   flash region. We read `bus.flash` (a `LinearMemory` whose `data`
//!   `Vec<u8>` never reallocates after config) directly instead — which is
//!   exactly the position-stable, flash-resident code the block cache
//!   requires. Extending `SystemBus::fetch_slice` to also serve `flash`/the
//!   C3 XIP peripheral is a later cleanup; it is not needed to prove the
//!   plumbing.
//! * **There is no flash-dirty flag on the bus.** Nothing tracks flash
//!   writes today, so [`take_flash_dirty`](JitHost::take_flash_dirty) is
//!   conservatively `false`. That is correct for any run that does not
//!   self-modify flash (the overwhelming common case, and every current
//!   test); when flash self-write support lands it wires in here.

use crate::cpu::RiscV;
use crate::Machine;

use super::super::fallback::{HostStep, JitHost, SafetyGate};
use super::super::{CodeView, Pc, StateVec};

/// Number of `u32` words in a RISC-V [`StateVec`]: `x0..x31` (32) + `pc`
/// (1) + the eight architectural M-mode CSRs (8).
pub const STATE_VEC_LEN: usize = 32 + 1 + 8;

/// Flatten a [`RiscV`] into the differential-harness [`StateVec`].
///
/// Layout (mirrors [`RiscV::snapshot`]'s field set, minus the volatile
/// `mtime`/`mtimecmp` CLINT words which are not architectural GPR/CSR
/// state):
///
/// | index | contents |
/// | ----- | -------- |
/// | `0..32` | `x0..x31` (x0 always reads 0) |
/// | `32` | `pc` |
/// | `33..41` | `mstatus, mie, mip, mtvec, mscratch, mepc, mcause, mtval` |
///
/// The cycle/`mtime`-derived CSRs (`0xC00`/`0x802`/`0x7E2`, …) are read
/// on demand from `mtime` and are deliberately **excluded**: a batched JIT
/// block advances `mtime` in one lump, so a mid-block sample of those would
/// differ from a per-instruction interpreter run. Excluding them (rather
/// than sampling and masking) keeps the vector to pure architectural state.
/// See also [`super::differential_cycle_ignore_indices`].
pub fn snapshot_state(cpu: &RiscV) -> StateVec {
    let mut v = Vec::with_capacity(STATE_VEC_LEN);
    v.extend_from_slice(&cpu.x); // x0..x31
    v.push(cpu.pc); // pc
    v.push(cpu.mstatus);
    v.push(cpu.mie);
    v.push(cpu.mip);
    v.push(cpu.mtvec);
    v.push(cpu.mscratch);
    v.push(cpu.mepc);
    v.push(cpu.mcause);
    v.push(cpu.mtval);
    debug_assert_eq!(v.len(), STATE_VEC_LEN);
    v
}

/// A [`JitHost`] view over a RISC-V machine, borrowing it for the lifetime
/// of one dispatch run.
pub struct RiscVJitHost<'m> {
    machine: &'m mut Machine<RiscV>,
}

impl<'m> RiscVJitHost<'m> {
    /// Wrap a machine for JIT dispatch.
    pub fn new(machine: &'m mut Machine<RiscV>) -> Self {
        Self { machine }
    }

    /// Borrow the underlying machine (telemetry / tests).
    pub fn machine(&self) -> &Machine<RiscV> {
        self.machine
    }
}

impl JitHost for RiscVJitHost<'_> {
    fn pc(&self) -> Pc {
        self.machine.cpu.pc as Pc
    }

    fn interpret_one(&mut self) -> HostStep {
        match self.machine.step() {
            Ok(()) => HostStep::Advanced,
            // A stopping condition (trap the interpreter cannot service,
            // decode error, halt): the dispatch loop returns. The
            // interpreter remains the single source of truth for *why*.
            Err(_) => HostStep::Halted,
        }
    }

    fn resume_at(&mut self, pc: Pc) {
        self.machine.cpu.pc = pc as u32;
    }

    fn code_view(&self, pc: Pc) -> Option<CodeView<'_>> {
        let flash = &self.machine.bus.flash;
        let base = flash.base_addr;
        let len = flash.data.len() as u64;
        if pc >= base && (pc - base) < len {
            Some(CodeView::new(base, &flash.data))
        } else {
            // Not flash-resident (RAM / MMIO / an XIP window we do not map
            // here) — not compilable; the PC stays on the interpreter.
            None
        }
    }

    fn safety(&self) -> SafetyGate {
        SafetyGate {
            // Any per-instruction observer forces interpretation.
            observers_active: !self.machine.observers.is_empty(),
            // A block could step over a breakpoint address without stopping.
            breakpoints_active: !self.machine.breakpoints.is_empty(),
            // Logic-analyzer / DAP-watch taps and cycle-accurate mode are
            // not surfaced through a stable accessor on this machine yet;
            // understating them is always safe (it only lets the JIT run
            // where it is in fact equivalent — the interpreter is the
            // reference), and the all-bail frontend is byte-identical
            // regardless. These wire in when the accessors land.
            probes_active: false,
            cycle_accurate: false,
        }
    }

    fn snapshot_state(&self) -> StateVec {
        snapshot_state(&self.machine.cpu)
    }

    fn take_flash_dirty(&mut self) -> bool {
        // No flash-write tracking on the bus (see module docs). Correct for
        // any run that does not self-modify flash.
        false
    }
}
