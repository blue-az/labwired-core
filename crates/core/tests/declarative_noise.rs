// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.

//! A declarative I²C descriptor with noise keys on an input channel produces
//! noisy (but seed-reproducible) register reads. Uses a TMP102-shaped inline
//! descriptor: TEMP at pointer 0x00, 16-bit BE, raw = °C × 16 (0.0625 °C/LSB).

use labwired_core::peripherals::components::declarative_i2c::GenericI2cDevice;
use labwired_core::peripherals::i2c::I2cDevice;
use labwired_core::sim_input::SimInput;

const NOISY_TMP102: &str = r#"
type: noisy_tmp102
behavior:
  primitive: i2c_device
  i2c:
    default_address: 0x48
    pointer_mask: 0x03
    registers:
      - { name: TEMP, addr: 0x00, width: 2, endian: be, access: r, reset: 0x1900, source: temperature, encode: { scale: 16.0 } }
metadata:
  label: "Noisy TMP102 test double"
  summary: "test"
  category: i2c
  inputs:
    - { key: temperature, label: "Temperature", unit: "°C", min: -40, max: 125, default: 25, noise_sigma: 0.25 }
"#;

fn build() -> GenericI2cDevice {
    GenericI2cDevice::from_yaml(NOISY_TMP102, 0).expect("builds")
}

fn read_temp_raw(dev: &mut GenericI2cDevice) -> u16 {
    dev.start();
    dev.write(0x00); // pointer = TEMP
    dev.start(); // repeated START: new read phase
    let msb = dev.read();
    let lsb = dev.read();
    dev.stop();
    ((msb as u16) << 8) | lsb as u16
}

#[test]
fn noisy_reads_differ_from_ideal_but_replay() {
    let mut a = build();
    let mut b = build();
    a.set_component_id("t0".into());
    b.set_component_id("t0".into());
    a.set_input("temperature", 25.0).unwrap();
    b.set_input("temperature", 25.0).unwrap();

    let ideal = (25.0_f64 * 16.0) as u16; // 0x0190
    let reads_a: Vec<u16> = (0..8).map(|_| read_temp_raw(&mut a)).collect();
    let reads_b: Vec<u16> = (0..8).map(|_| read_temp_raw(&mut b)).collect();

    assert!(
        reads_a.iter().any(|&r| r != ideal),
        "noise never moved the reading: {reads_a:?}"
    );
    // σ = 0.25 °C = 4 LSB; 40 LSB is 10σ — generous but catches runaway gain.
    for &r in &reads_a {
        let delta = (r as i32 - ideal as i32).abs();
        assert!(delta < 40, "reading {r:#x} unreasonably far from {ideal:#x}");
    }
    assert_eq!(reads_a, reads_b, "same seed must replay bit-identically");
}

#[test]
fn word_bytes_carry_one_observation() {
    // MSB and LSB of a single read come from the same noise sample: re-reading
    // only the low byte (no new read phase) never shows a different sample.
    let mut dev = build();
    dev.set_component_id("t0".into());
    dev.set_input("temperature", 25.0).unwrap();
    dev.start();
    dev.write(0x00);
    dev.start();
    let msb = dev.read();
    let lsb = dev.read();
    dev.stop();
    let combined = ((msb as u16) << 8) | lsb as u16;
    let ideal = (25.0_f64 * 16.0) as i32;
    assert!(
        (combined as i32 - ideal).abs() < 40,
        "combined {combined:#x} unreasonable"
    );
}

#[test]
fn no_noise_keys_stays_exact() {
    // Same descriptor minus noise keys → byte-identical to pre-noise behavior.
    let plain = NOISY_TMP102.replace(", noise_sigma: 0.25", "");
    let mut dev = GenericI2cDevice::from_yaml(&plain, 0).expect("builds");
    dev.set_component_id("t0".into());
    dev.set_input("temperature", 25.0).unwrap();
    for _ in 0..4 {
        assert_eq!(read_temp_raw(&mut dev), 0x0190);
    }
}

#[test]
fn distinct_components_diverge() {
    let mut a = build();
    let mut b = build();
    a.set_component_id("temp-a".into());
    b.set_component_id("temp-b".into());
    a.set_input("temperature", 25.0).unwrap();
    b.set_input("temperature", 25.0).unwrap();
    let reads_a: Vec<u16> = (0..8).map(|_| read_temp_raw(&mut a)).collect();
    let reads_b: Vec<u16> = (0..8).map(|_| read_temp_raw(&mut b)).collect();
    assert_ne!(reads_a, reads_b, "different ids must re-key the noise");
}
