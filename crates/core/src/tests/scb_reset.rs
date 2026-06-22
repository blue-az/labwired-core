// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.

#[cfg(test)]
mod scb_reset_tests {
    use crate::{Cpu, Machine};

    /// AIRCR address: SCB base (0xE000_ED00) + 0x0C.
    const SCB_AIRCR: u64 = 0xE000_ED0C;

    /// A real Cortex-M write to AIRCR with the correct VECTKEY (0x05FA in
    /// bits 31:16) and SYSRESETREQ (bit 2) must reboot the CPU through the
    /// vector table on the next instruction boundary: MSP reloads from
    /// vector[0] and PC from vector[1] (with the Thumb bit masked off),
    /// reusing the existing power-on reset path.
    #[test]
    fn sysresetreq_reboots_cpu_via_vector_table() {
        // Bare Cortex-M machine. `configure_cortex_m` registers the SCB at
        // 0xE000_ED00 with VTOR defaulting to 0, so the vector table lives at
        // address 0 (vector[0]=MSP, vector[1]=reset).
        let mut bus = crate::bus::SystemBus::new();
        let (cpu, _nvic) = crate::system::cortex_m::configure_cortex_m(&mut bus);
        let mut m = Machine::new(cpu, bus);

        const MSP: u32 = 0x2000_1000;
        const RESET_ADDR: u32 = 0x0800_0100;

        // Seed the vector table the same way power-on reset reads it.
        m.bus.write_u32(0x0000_0000, MSP).unwrap();
        m.bus.write_u32(0x0000_0004, RESET_ADDR | 1).unwrap(); // Thumb bit set

        // Place a harmless instruction (NOP, 0xBF00) at the current PC so the
        // step executes one full instruction before the reset latch is drained
        // — mirroring firmware whose AIRCR store completes, then the core
        // reboots at the next instruction boundary.
        const PC: u32 = 0x2000_0000;
        m.bus.write_u16(PC as u64, 0xBF00).unwrap(); // NOP
        m.cpu.set_pc(PC);
        m.cpu.set_sp(0x2000_8000);

        // Trigger the latch through the exact MMIO path firmware uses: a real
        // bus write to AIRCR. No test-only Scb setter.
        m.bus
            .write_u32(SCB_AIRCR, (0x05FA << 16) | (1 << 2))
            .unwrap();

        // One step: execute the NOP, then drain the SCB latch and reset.
        m.step().unwrap();

        assert_eq!(
            m.cpu.get_pc() & !1,
            RESET_ADDR,
            "PC must reload from vector[1] (reset vector) after SYSRESETREQ"
        );
        assert_eq!(
            m.cpu.get_register(13),
            MSP,
            "SP must reload from vector[0] (MSP) after SYSRESETREQ"
        );
    }

    /// An AIRCR write missing the VECTKEY must NOT reset the CPU: the latch is
    /// never set, so `step()` leaves PC/SP advancing normally.
    #[test]
    fn aircr_without_vectkey_does_not_reboot() {
        let mut bus = crate::bus::SystemBus::new();
        let (cpu, _nvic) = crate::system::cortex_m::configure_cortex_m(&mut bus);
        let mut m = Machine::new(cpu, bus);

        const MSP: u32 = 0x2000_1000;
        const RESET_ADDR: u32 = 0x0800_0100;
        m.bus.write_u32(0x0000_0000, MSP).unwrap();
        m.bus.write_u32(0x0000_0004, RESET_ADDR | 1).unwrap();

        const PC: u32 = 0x2000_0000;
        m.bus.write_u16(PC as u64, 0xBF00).unwrap(); // NOP
        m.cpu.set_pc(PC);
        m.cpu.set_sp(0x2000_8000);

        // SYSRESETREQ bit set but no VECTKEY — silicon ignores it.
        m.bus.write_u32(SCB_AIRCR, 1 << 2).unwrap();

        m.step().unwrap();

        assert_ne!(
            m.cpu.get_pc() & !1,
            RESET_ADDR,
            "PC must not jump to the reset vector without the VECTKEY"
        );
        assert_eq!(
            m.cpu.get_register(13),
            0x2000_8000,
            "SP must be untouched without a valid reset request"
        );
    }
}
