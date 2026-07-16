use crate::runtime_snapshot::CpuKind;
use crate::snapshot::{ArmCpuSnapshot, CpuSnapshot};
use crate::{
    Bus, Cpu, DebugControl, Machine, SimResult, SimulationConfig, SimulationObserver, StopReason,
};
use std::sync::Arc;

#[derive(Debug, Default)]
struct CountingCpu {
    pc: u32,
    sp: u32,
    steps: u32,
    pending: Vec<u64>,
    halted: bool,
}

impl Cpu for CountingCpu {
    fn reset(&mut self, _bus: &mut dyn Bus) -> SimResult<()> {
        self.pc = 0;
        self.sp = 0;
        self.steps = 0;
        self.pending.clear();
        self.halted = false;
        Ok(())
    }

    fn step(
        &mut self,
        _bus: &mut dyn Bus,
        _observers: &[Arc<dyn SimulationObserver>],
        _config: &SimulationConfig,
    ) -> SimResult<()> {
        if !self.halted {
            self.steps += 1;
            self.pc = self.pc.wrapping_add(2);
        }
        Ok(())
    }

    fn set_pc(&mut self, val: u32) {
        self.pc = val;
    }

    fn get_pc(&self) -> u32 {
        self.pc
    }

    fn set_sp(&mut self, val: u32) {
        self.sp = val;
    }

    fn set_exception_pending(&mut self, exception_num: u32) {
        let word = exception_num as usize / 64;
        let bit = exception_num % 64;
        if self.pending.len() <= word {
            self.pending.resize(word + 1, 0);
        }
        self.pending[word] |= 1_u64 << bit;
    }

    fn get_register(&self, id: u8) -> u32 {
        match id {
            0 => self.steps,
            13 => self.sp,
            15 => self.pc,
            _ => 0,
        }
    }

    fn set_register(&mut self, id: u8, val: u32) {
        match id {
            0 => self.steps = val,
            13 => self.sp = val,
            15 => self.pc = val,
            _ => {}
        }
    }

    fn snapshot(&self) -> CpuSnapshot {
        let mut registers = vec![0; 16];
        registers[0] = self.steps;
        registers[13] = self.sp;
        registers[15] = self.pc;
        CpuSnapshot::Arm(ArmCpuSnapshot {
            registers,
            pc: self.pc,
            xpsr: 0,
            primask: false,
            pending_exceptions: self.pending.first().copied().unwrap_or(0),
            pending_exceptions_hi: self.pending.iter().skip(1).copied().collect(),
            vtor: 0,
        })
    }

    fn apply_snapshot(&mut self, snapshot: &CpuSnapshot) {
        if let CpuSnapshot::Arm(snapshot) = snapshot {
            self.steps = snapshot.registers.first().copied().unwrap_or(0);
            self.sp = snapshot.registers.get(13).copied().unwrap_or(0);
            self.pc = snapshot.pc;
            self.pending.clear();
            self.pending.push(snapshot.pending_exceptions);
            self.pending
                .extend(snapshot.pending_exceptions_hi.iter().copied());
        }
    }

    fn runtime_snapshot(&self) -> (CpuKind, Vec<u8>) {
        let state = (
            self.pc,
            self.sp,
            self.steps,
            self.pending.clone(),
            self.halted,
        );
        (
            CpuKind::ArmCortexM,
            bincode::serialize(&state).unwrap_or_default(),
        )
    }

    fn apply_runtime_snapshot(&mut self, kind: CpuKind, bytes: &[u8]) -> SimResult<()> {
        if kind == CpuKind::ArmCortexM {
            if let Ok((pc, sp, steps, pending, halted)) =
                bincode::deserialize::<(u32, u32, u32, Vec<u64>, bool)>(bytes)
            {
                self.pc = pc;
                self.sp = sp;
                self.steps = steps;
                self.pending = pending;
                self.halted = halted;
            }
        }
        Ok(())
    }

    fn get_register_names(&self) -> Vec<String> {
        (0..16).map(|id| format!("r{id}")).collect()
    }

    fn index_of_register(&self, name: &str) -> Option<u8> {
        name.strip_prefix('r')?
            .parse::<u8>()
            .ok()
            .filter(|id| *id < 16)
    }

    fn halt(&mut self) {
        self.halted = true;
    }

    fn unhalt(&mut self) {
        self.halted = false;
    }
}

fn counting_dual_core_machine() -> Machine<CountingCpu> {
    Machine::new(CountingCpu::default(), crate::bus::SystemBus::new())
        .with_secondary_cpu(CountingCpu::default())
}

#[test]
fn legacy_step_advances_both_cores_once() {
    let mut machine = counting_dual_core_machine();

    machine.step().expect("legacy step should succeed");

    assert_eq!(machine.cpu.steps, 1);
    assert_eq!(machine.cpu_secondary.as_ref().map(|cpu| cpu.steps), Some(1));
    assert_eq!(machine.total_cycles, 1);
}

#[test]
fn legacy_run_currently_omits_secondary_core() {
    let mut machine = counting_dual_core_machine();

    let reason = machine.run(Some(4)).expect("legacy run should succeed");

    assert_eq!(reason, StopReason::MaxStepsReached);
    assert_eq!(machine.cpu.steps, 4);
    assert_eq!(machine.cpu_secondary.as_ref().map(|cpu| cpu.steps), Some(0));
}

#[test]
fn legacy_single_step_publishes_and_profiles_one_cycle() {
    let mut machine = Machine::new(CountingCpu::default(), crate::bus::SystemBus::new());

    machine.step().expect("legacy step should succeed");

    assert_eq!(machine.total_cycles, 1);
    assert_eq!(machine.bus.current_cycle, 1);
    let profile = machine.step_profile();
    assert_eq!(profile.cpu_instructions, 1);
    assert_eq!(profile.cpu_batches, 1);
}

#[test]
fn legacy_dual_core_halted_primary_still_consumes_one_scheduling_quantum() {
    let mut machine = counting_dual_core_machine();
    machine.cpu.halt();

    machine.step().expect("legacy step should succeed");

    assert_eq!(machine.cpu.steps, 0);
    assert_eq!(machine.cpu_secondary.as_ref().map(|cpu| cpu.steps), Some(1));
    assert_eq!(machine.total_cycles, 1);
    assert_eq!(machine.step_profile().cpu_instructions, 1);
}
