// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

use labwired_core::system::cortex_m::configure_cortex_m;
use labwired_core::{Bus, Machine};
use std::sync::{Arc, Mutex};

fn fixture(
    stalled: bool,
) -> (
    Machine<labwired_core::cpu::cortex_m::CortexM>,
    Arc<Mutex<Vec<u8>>>,
) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut bus = labwired_core::system::builder::build_system_bus(Some(
        &root.join("examples/nucleo-l476rg-bldc/system.yaml"),
    ))
    .unwrap();
    let uart = Arc::new(Mutex::new(Vec::new()));
    bus.attach_uart_tx_sink(uart.clone(), false);
    let (cpu, _) = configure_cortex_m(&mut bus);
    let mut machine = Machine::new(cpu, bus);
    let elf = root.join("target/thumbv7em-none-eabihf/release/firmware-l476-bldc-six-step");
    let image = labwired_loader::load_elf(&elf)
        .unwrap_or_else(|error| panic!("build firmware fixture first ({elf:?}): {error}"));
    machine.load_firmware(&image).unwrap();
    if stalled {
        machine.set_input_on("drive_motor", "stall", 1.0).unwrap();
    }
    (machine, uart)
}

fn contains(uart: &Arc<Mutex<Vec<u8>>>, token: &[u8]) -> bool {
    uart.lock()
        .unwrap()
        .windows(token.len())
        .any(|bytes| bytes == token)
}

#[test]
fn l476_bldc_stall_runs_real_firmware_and_disables_real_inverter() {
    let (mut machine, uart) = fixture(false);
    for _ in 0..500_000 {
        machine.step().unwrap();
        if contains(&uart, b"TARGET REACHED") {
            break;
        }
    }
    assert!(
        contains(&uart, b"TARGET REACHED"),
        "uart={}",
        format!(
            "{} snapshot={:?}",
            String::from_utf8_lossy(&uart.lock().unwrap()),
            (
                machine.bus.motor_snapshots(),
                machine.bus.current_cycle,
                machine.bus.read_u32(0x4001_2c20).unwrap(),
                machine.bus.read_u32(0x4001_2c34).unwrap(),
                machine.bus.read_u32(0x4001_2c3c).unwrap(),
                machine.bus.read_u32(0xe000_e010).unwrap()
            )
        )
    );
    let acquired = machine.bus.motor_snapshots().remove(0);
    assert!(
        acquired.speed_rpm.abs() > 1.0,
        "target token requires plant motion"
    );

    let injected_cycle = machine.bus.current_cycle;
    machine.set_input_on("drive_motor", "stall", 1.0).unwrap();
    for _ in 0..500_000 {
        machine.step().unwrap();
        if contains(&uart, b"INVERTER OFF") {
            break;
        }
    }
    let shutdown_cycle = machine.bus.current_cycle;
    assert!(contains(&uart, b"FAULT STALL"));
    assert!(contains(&uart, b"INVERTER OFF"));
    assert!(shutdown_cycle - injected_cycle <= 300_000);
    let stopped = machine.bus.motor_snapshots().remove(0);
    assert!(stopped.faults.iter().any(|fault| fault == "stalled"));
    assert_eq!(machine.bus.read_u32(0x4001_2c44).unwrap() & (1 << 15), 0);
    assert_eq!(machine.bus.read_u32(0x4800_0414).unwrap() & 1, 0);
}

#[test]
fn l476_bldc_never_reports_target_without_rotor_motion() {
    let (mut machine, uart) = fixture(true);
    for _ in 0..500_000 {
        machine.step().unwrap();
        if contains(&uart, b"INVERTER OFF") {
            break;
        }
    }
    assert!(contains(&uart, b"FAULT STALL"));
    assert!(!contains(&uart, b"TARGET REACHED"));
    assert_eq!(machine.bus.motor_snapshots()[0].speed_rpm, 0.0);
}
