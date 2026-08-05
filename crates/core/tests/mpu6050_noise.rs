// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.

use labwired_core::peripherals::components::Mpu6050;
use labwired_core::peripherals::i2c::I2cDevice;
use labwired_core::sim_input::SimInput;

fn read_reg(dev: &mut Mpu6050, reg: u8) -> u8 {
    dev.write(reg);
    dev.stop();
    let v = dev.read();
    dev.stop();
    v
}

#[test]
fn noise_moves_accel_reads_and_replays() {
    let mut a = Mpu6050::new(0x68).with_noise_sigma(0.02);
    let mut b = Mpu6050::new(0x68).with_noise_sigma(0.02);
    for d in [&mut a, &mut b] {
        SimInput::set_component_id(d, "imu".into());
        SimInput::set_input(d, "ax", 1.0).unwrap();
    }
    let ra: Vec<u8> = (0..8).map(|_| read_reg(&mut a, 0x3B)).collect();
    let rb: Vec<u8> = (0..8).map(|_| read_reg(&mut b, 0x3B)).collect();
    // 1 g at ±2g = 16384 counts; σ = 0.02 g ≈ 328 counts → MSB must move sometimes.
    let ideal_msb = (16384u16 >> 8) as u8; // 0x40
    assert!(
        ra.iter().any(|&r| r != ideal_msb),
        "noise never moved the MSB: {ra:?}"
    );
    assert_eq!(ra, rb, "same seed must replay bit-identically");
}

#[test]
fn no_noise_config_is_byte_identical_to_before() {
    let mut dev = Mpu6050::new(0x68);
    SimInput::set_input(&mut dev, "ax", 1.0).unwrap();
    for _ in 0..4 {
        assert_eq!(read_reg(&mut dev, 0x3B), 0x40);
        assert_eq!(read_reg(&mut dev, 0x3C), 0x00);
    }
}

#[test]
fn noise_sigma_zero_is_byte_identical() {
    let mut dev = Mpu6050::new(0x68).with_noise_sigma(0.0);
    SimInput::set_component_id(&mut dev, "imu".into());
    SimInput::set_input(&mut dev, "ax", 1.0).unwrap();
    for _ in 0..4 {
        assert_eq!(read_reg(&mut dev, 0x3B), 0x40);
    }
}
