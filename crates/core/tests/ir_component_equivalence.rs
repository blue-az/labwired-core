// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! Byte-equivalence gate: the IR-interpreted PCA9685 must be
//! indistinguishable from the hand-written Rust model over the I2cDevice
//! interface — same bytes read, same observables — across a deterministic
//! transaction corpus including the firmware's dispense sequences.

use labwired_core::peripherals::components::{IrI2cComponent, Pca9685};
use labwired_core::peripherals::i2c::I2cDevice;

fn ir_pca() -> IrI2cComponent {
    let yaml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../configs/components/pca9685.yaml"
    ))
    .expect("spec asset");
    IrI2cComponent::new(serde_yaml::from_str(&yaml).expect("parse"), None).expect("valid")
}

/// One bus op. Deterministic corpus only — no randomness.
enum Op {
    Start,
    Write(u8),
    Read,
}

fn run_corpus(ops: &[Op]) {
    let mut rust: Box<dyn I2cDevice> = Box::new(Pca9685::new());
    let mut ir = ir_pca();
    assert_eq!(rust.address(), ir.address(), "address");
    for (i, op) in ops.iter().enumerate() {
        match op {
            Op::Start => {
                rust.start();
                ir.start();
            }
            Op::Write(b) => {
                rust.write(*b);
                ir.write(*b);
            }
            Op::Read => {
                assert_eq!(rust.read(), ir.read(), "read divergence at op {i}");
            }
        }
    }
    // Observables must agree with the Rust model's accessors on every channel.
    let rust_concrete = rust.as_any().unwrap().downcast_ref::<Pca9685>().unwrap();
    for ch in 0..16u8 {
        let a = rust_concrete.channel_angle_deg(ch);
        let b = ir.observable("servo_angle", ch);
        match (a, b) {
            (None, None) => {}
            (Some(x), Some(y)) => assert!((x - y).abs() < 0.01, "ch {ch}: {x} vs {y}"),
            _ => panic!("ch {ch}: presence mismatch {a:?} vs {b:?}"),
        }
    }
}

fn set_angle_ops(ops: &mut Vec<Op>, ch: u8, deg: f64) {
    let us = 500.0 + (deg / 180.0) * 1900.0;
    let ticks = (us / 20000.0 * 4096.0) as u16;
    ops.push(Op::Start);
    ops.push(Op::Write(0x06 + 4 * ch));
    ops.push(Op::Write(0x00));
    ops.push(Op::Write(0x00));
    ops.push(Op::Write((ticks & 0xFF) as u8));
    ops.push(Op::Write(((ticks >> 8) & 0x0F) as u8));
}

#[test]
fn dispense_sequence_is_byte_equivalent() {
    let mut ops = vec![Op::Start, Op::Write(0x00), Op::Write(0xA1)]; // AI on
    set_angle_ops(&mut ops, 8, 15.0); // revolver → compartment 1
    set_angle_ops(&mut ops, 12, 20.0); // shutter closed
    set_angle_ops(&mut ops, 12, 90.0); // shutter open
    set_angle_ops(&mut ops, 8, 135.0); // revolver → compartment 5
                                       // Read back the channel-8 block through AI.
    ops.push(Op::Start);
    ops.push(Op::Write(0x06 + 4 * 8));
    for _ in 0..4 {
        ops.push(Op::Read);
    }
    run_corpus(&ops);
}

#[test]
fn pointer_semantics_without_ai_are_byte_equivalent() {
    // AI off (power-on MODE1=0x11): repeated reads hit the same register.
    let ops = vec![
        Op::Start,
        Op::Write(0x00), // pointer = MODE1
        Op::Read,
        Op::Read,
        Op::Start,
        Op::Write(0x06),
        Op::Write(0x55), // data write with AI off
        Op::Write(0x66), // overwrites same register
        Op::Start,
        Op::Write(0x06),
        Op::Read,
    ];
    run_corpus(&ops);
}

#[test]
fn full_register_sweep_is_byte_equivalent() {
    // Walk every register: write a deterministic pattern with AI on, then
    // read the whole file back and compare byte-for-byte.
    let mut ops = vec![Op::Start, Op::Write(0x00), Op::Write(0xA1)];
    ops.push(Op::Start);
    ops.push(Op::Write(0x01)); // start after MODE1 to keep AI set
    for i in 1..=255u32 {
        ops.push(Op::Write((i.wrapping_mul(37) & 0xFF) as u8));
    }
    ops.push(Op::Start);
    ops.push(Op::Write(0x00));
    for _ in 0..=255 {
        ops.push(Op::Read);
    }
    run_corpus(&ops);
}
