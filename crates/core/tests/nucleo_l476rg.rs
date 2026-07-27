// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

use labwired_core::system::builder::build_system_bus;
use labwired_core::Bus;

#[test]
fn l476_bldc_stall_is_firmware_visible_and_moe_shutdown_disables_inverter() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut bus =
        build_system_bus(Some(&root.join("examples/nucleo-l476rg-bldc/system.yaml"))).unwrap();

    bus.write_u32(0x4002_104c, 1).unwrap(); // RCC GPIOA clock
    bus.write_u32(0x4002_1060, 1 << 11).unwrap(); // RCC TIM1 clock
    bus.write_u32(0x4800_0014, 1).unwrap(); // PA0 external enable
    bus.write_u32(0x4001_2c2c, 999).unwrap();
    bus.write_u32(0x4001_2c34, 300).unwrap();
    bus.write_u32(0x4001_2c38, 300).unwrap();
    bus.write_u32(0x4001_2c3c, 300).unwrap();
    bus.write_u32(0x4001_2c18, 0x6868).unwrap();
    bus.write_u32(0x4001_2c1c, 0x0068).unwrap();
    bus.write_u32(0x4001_2c20, (1 << 0) | (1 << 6)).unwrap();
    bus.write_u32(0x4001_2c44, (1 << 15) | 0x30).unwrap();
    bus.write_u32(0x4001_2c00, 1).unwrap();

    bus.set_input(Some("drive_motor"), "stall", 1.0).unwrap();
    bus.set_current_cycle(4096);
    bus.tick_peripherals_with_costs();
    assert_ne!(
        bus.read_u32(0x4800_0010).unwrap() & (1 << 7),
        0,
        "motor fault must reach PA7"
    );

    // The firmware fault path performs these writes in this order.
    bus.write_u32(0x4001_2c44, 0x30).unwrap(); // clear BDTR.MOE
    bus.write_u32(0x4001_2c20, 0).unwrap(); // no commanded leg
    bus.write_u32(0x4800_0014, 0).unwrap(); // external enable low
    bus.set_current_cycle(8192);
    bus.tick_peripherals_with_costs();
    let snapshot = bus.motor_snapshots().remove(0);
    assert_eq!(snapshot.control_state, "fault:motor");
    assert!(snapshot.faults.iter().any(|fault| fault == "stalled"));
    assert_eq!(bus.read_u32(0x4001_2c44).unwrap() & (1 << 15), 0);
    assert_eq!(bus.read_u32(0x4800_0014).unwrap() & 1, 0);
}
