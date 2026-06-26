// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! Sensirion **SPS30** particulate-matter sensor as an [`I2cDevice`].
//!
//! The SPS30 reports mass and number concentrations across particle-size bins.
//! Datasheet (SPS30, Sensirion, rev 1.0) protocol — 16-bit big-endian commands,
//! responses are 16-bit words each followed by a CRC-8 (poly 0x31) byte:
//! - `0x0010` start_measurement  (param word: `0x0300` float / `0x0500` uint16)
//! - `0x0104` stop_measurement   (no response)
//! - `0x0202` read_data_ready    → 1 word: ready when low byte = 0x01
//! - `0x0300` read_measured_values → 10 values (float mode: each value is 2
//!   words/4 bytes + 2 CRCs; uint16 mode: each value is 1 word + CRC). Order:
//!   mass PM1.0/2.5/4.0/10.0, number PM0.5/1.0/2.5/4.0/10.0, typical size.
//! - `0xD002` reset / `0x5607` wake / `0x5602` sleep / `0xD060` fan-clean (no resp)
//! - `0xD033` read serial → 48 bytes (16 words)
//!
//! PM2.5 mass advances along a [`Ramp`]; the other bins are derived from it with
//! fixed ratios so the whole particle picture moves together.

use crate::peripherals::components::air_scene::Ramp;
use crate::peripherals::components::sensirion::{crc8, encode_words};
use crate::peripherals::i2c::I2cDevice;

pub const SPS30_ADDR: u8 = 0x69;

const CMD_START_MEASUREMENT: u16 = 0x0010;
const CMD_STOP_MEASUREMENT: u16 = 0x0104;
const CMD_READ_DATA_READY: u16 = 0x0202;
const CMD_READ_MEASURED_VALUES: u16 = 0x0300;
const CMD_GET_SERIAL: u16 = 0xD033;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Float,
    Uint16,
}

/// SPS30 model.
pub struct Sps30 {
    address: u8,
    pm2_5: Ramp,
    mode: Mode,
    running: bool,
    write_buf: Vec<u8>,
    read_buf: Vec<u8>,
    read_idx: usize,
}

impl Sps30 {
    /// `pm2_5_start`/`pm2_5_target` in µg/m³; `alpha` per-read ramp rate.
    pub fn new(address: u8, pm2_5_start: f64, pm2_5_target: f64, alpha: f64) -> Self {
        let address = if address == 0 { SPS30_ADDR } else { address };
        Self {
            address,
            pm2_5: Ramp::new(pm2_5_start, pm2_5_target, alpha),
            mode: Mode::Float,
            running: false,
            write_buf: Vec::with_capacity(8),
            read_buf: Vec::new(),
            read_idx: 0,
        }
    }

    pub fn new_default(address: u8) -> Self {
        Self::new(address, 6.0, 22.0, 0.08)
    }

    /// Encode one IEEE-754 float as the SPS30 clocks it: big-endian, split into
    /// two 16-bit words, each followed by its CRC.
    fn push_float(buf: &mut Vec<u8>, value: f32) {
        let b = value.to_be_bytes();
        for word in [[b[0], b[1]], [b[2], b[3]]] {
            buf.push(word[0]);
            buf.push(word[1]);
            buf.push(crc8(&word));
        }
    }

    /// Build the 10-value measurement payload from the current PM2.5 ramp.
    fn measured_values(&mut self) -> Vec<u8> {
        let pm2_5 = self.pm2_5.advance().max(0.0);
        // Fixed bin ratios (typical urban aerosol) so all bins track PM2.5.
        let pm1_0 = pm2_5 * 0.78;
        let pm4_0 = pm2_5 * 1.10;
        let pm10 = pm2_5 * 1.18;
        // Number concentrations (#/cm³) scale roughly with mass.
        let n0_5 = pm2_5 * 3.5;
        let n1_0 = pm2_5 * 4.0;
        let n2_5 = pm2_5 * 4.1;
        let n4_0 = pm2_5 * 4.15;
        let n10 = pm2_5 * 4.2;
        let typ_size = 0.55_f64; // µm, typical

        let values = [
            pm1_0, pm2_5, pm4_0, pm10, n0_5, n1_0, n2_5, n4_0, n10, typ_size,
        ];

        match self.mode {
            Mode::Float => {
                let mut buf = Vec::with_capacity(60);
                for v in values {
                    Self::push_float(&mut buf, v as f32);
                }
                buf
            }
            Mode::Uint16 => {
                // uint16 mode: mass ×10? No — datasheet sends integers directly
                // (µg/m³, #/cm³, nm for size). Size in nm to keep precision.
                let words: Vec<u16> = values
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| {
                        if i == 9 {
                            (v * 1000.0).round().clamp(0.0, 65535.0) as u16 // µm → nm
                        } else {
                            v.round().clamp(0.0, 65535.0) as u16
                        }
                    })
                    .collect();
                encode_words(&words)
            }
        }
    }

    fn dispatch(&mut self, cmd: u16) {
        self.read_buf.clear();
        self.read_idx = 0;
        match cmd {
            CMD_START_MEASUREMENT => {
                self.running = true;
                // Mode is decided by the param word that follows; default float.
                self.mode = Mode::Float;
            }
            CMD_STOP_MEASUREMENT => self.running = false,
            CMD_READ_DATA_READY => {
                // Low byte 0x01 ⇒ a new measurement is available.
                self.read_buf = encode_words(&[0x0001]);
            }
            CMD_READ_MEASURED_VALUES => {
                self.read_buf = self.measured_values();
            }
            CMD_GET_SERIAL => {
                // 16 words of ASCII-ish serial; first spells "LEO".
                let mut words = vec![0x4C45, 0x4F30, 0x3100];
                words.resize(16, 0x0000);
                self.read_buf = encode_words(&words);
            }
            _ => {}
        }
    }
}

impl I2cDevice for Sps30 {
    fn address(&self) -> u8 {
        self.address
    }

    fn start(&mut self) {
        self.write_buf.clear();
        self.read_idx = 0;
    }

    fn stop(&mut self) {
        // Sensirion command/read are separate transactions; clear the command
        // accumulator at transaction end so the next command dispatches.
        self.write_buf.clear();
    }

    fn write(&mut self, data: u8) {
        self.write_buf.push(data);
        match self.write_buf.len() {
            2 => {
                let cmd = ((self.write_buf[0] as u16) << 8) | (self.write_buf[1] as u16);
                self.dispatch(cmd);
            }
            // start_measurement carries a mode param word at bytes [2..4].
            4 => {
                let cmd = ((self.write_buf[0] as u16) << 8) | (self.write_buf[1] as u16);
                if cmd == CMD_START_MEASUREMENT {
                    self.mode = if self.write_buf[2] == 0x05 {
                        Mode::Uint16
                    } else {
                        Mode::Float
                    };
                }
            }
            _ => {}
        }
    }

    fn read(&mut self) -> u8 {
        let byte = self.read_buf.get(self.read_idx).copied().unwrap_or(0xFF);
        self.read_idx += 1;
        byte
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(d: &mut Sps30, bytes: &[u8]) {
        d.start();
        for &b in bytes {
            d.write(b);
        }
    }

    fn read_n(d: &mut Sps30, n: usize) -> Vec<u8> {
        d.start();
        (0..n).map(|_| d.read()).collect()
    }

    /// Decode an SPS30 float value (2 words + CRCs) at byte offset `i*6`.
    fn decode_float(buf: &[u8], idx: usize) -> f32 {
        let o = idx * 6;
        let bytes = [buf[o], buf[o + 1], buf[o + 3], buf[o + 4]];
        f32::from_be_bytes(bytes)
    }

    #[test]
    fn address_defaults_to_0x69() {
        assert_eq!(Sps30::new_default(0).address(), 0x69);
    }

    #[test]
    fn float_mode_returns_60_bytes_with_valid_crcs() {
        let mut d = Sps30::new_default(SPS30_ADDR);
        send(&mut d, &[0x00, 0x10, 0x03, 0x00, crc8(&[0x03, 0x00])]); // start float
        send(&mut d, &[0x03, 0x00]); // read_measured_values
        let b = read_n(&mut d, 60);
        assert_eq!(b.len(), 60);
        for chunk in b.chunks(3) {
            assert_eq!(chunk[2], crc8(&chunk[..2]), "valid CRC per word");
        }
    }

    #[test]
    fn pm2_5_is_second_value_and_climbs() {
        let mut d = Sps30::new_default(SPS30_ADDR);
        send(&mut d, &[0x00, 0x10, 0x03, 0x00, crc8(&[0x03, 0x00])]);
        let mut first = 0.0f32;
        let mut last = 0.0f32;
        for i in 0..60 {
            send(&mut d, &[0x03, 0x00]);
            let b = read_n(&mut d, 60);
            let pm2_5 = decode_float(&b, 1); // index 1 = PM2.5 mass
            if i == 0 {
                first = pm2_5;
            }
            last = pm2_5;
        }
        assert!(first >= 5.0 && first < 10.0, "starts fresh-ish: {first}");
        assert!(last > 18.0, "PM2.5 climbs toward target: {last}");
    }

    #[test]
    fn data_ready_flag_reports_ready() {
        let mut d = Sps30::new_default(SPS30_ADDR);
        send(&mut d, &[0x02, 0x02]);
        let b = read_n(&mut d, 3);
        assert_eq!(b[1], 0x01, "low byte 1 = data ready");
        assert_eq!(b[2], crc8(&b[..2]));
    }

    #[test]
    fn uint16_mode_returns_30_bytes() {
        let mut d = Sps30::new_default(SPS30_ADDR);
        send(&mut d, &[0x00, 0x10, 0x05, 0x00, crc8(&[0x05, 0x00])]); // start uint16
        send(&mut d, &[0x03, 0x00]);
        let b = read_n(&mut d, 30);
        assert_eq!(b.len(), 30);
        for chunk in b.chunks(3) {
            assert_eq!(chunk[2], crc8(&chunk[..2]));
        }
    }
}
