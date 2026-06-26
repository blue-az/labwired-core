// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! Vishay **VEML7700** ambient-light sensor as an [`I2cDevice`].
//!
//! Unlike the Sensirion parts on this board, the VEML7700 is a classic
//! command-register device: the master writes a 1-byte register pointer, then
//! reads/writes a **16-bit little-endian** word (low byte first).
//!
//! Datasheet (VEML7700, Vishay, rev 1.6) register map:
//! - `0x00` ALS_CONF   (config: gain, integration time, power)
//! - `0x01` ALS_WH / `0x02` ALS_WL (window thresholds)
//! - `0x03` PSM        (power-saving mode)
//! - `0x04` ALS        (ambient-light output, 16-bit) — read
//! - `0x05` WHITE      (white-channel output, 16-bit) — read
//!
//! Lux = `raw_counts × resolution`. At the power-on default (gain ×1,
//! integration time 100 ms) resolution is `0.0576 lux/count`, so this model
//! inverts a lux [`Ramp`] into raw counts the firmware multiplies back out.
//! The default scenario *dims* (450 → 90 lux) as the room settles for the
//! evening, complementing the rising CO₂ story.

use crate::peripherals::components::air_scene::Ramp;
use crate::peripherals::i2c::I2cDevice;

pub const VEML7700_ADDR: u8 = 0x10;

/// Lux per count at the default gain ×1 / integration-time 100 ms.
const RESOLUTION_LUX_PER_COUNT: f64 = 0.0576;

const REG_ALS: u8 = 0x04;
const REG_WHITE: u8 = 0x05;

/// VEML7700 model.
pub struct Veml7700 {
    address: u8,
    lux: Ramp,
    /// Config registers the firmware writes (ALS_CONF etc.), echoed back on read.
    conf: [u16; 4],
    /// Selected register pointer for the current transaction.
    pointer: u8,
    /// Bytes written this transaction (pointer + optional data low/high).
    write_buf: Vec<u8>,
    /// Latched 16-bit read word (LE) for the current read, and which byte is next.
    read_word: u16,
    read_byte_idx: usize,
    /// Whether the current read has latched its word yet (advance exactly once).
    latched: bool,
}

impl Veml7700 {
    /// `lux_start`/`lux_target` in lux; `alpha` per-read ramp rate (light dims
    /// when `target < start`).
    pub fn new(address: u8, lux_start: f64, lux_target: f64, alpha: f64) -> Self {
        let address = if address == 0 { VEML7700_ADDR } else { address };
        Self {
            address,
            lux: Ramp::new(lux_start, lux_target, alpha),
            conf: [0; 4],
            pointer: 0,
            write_buf: Vec::with_capacity(4),
            read_word: 0,
            read_byte_idx: 0,
            latched: false,
        }
    }

    pub fn new_default(address: u8) -> Self {
        Self::new(address, 450.0, 90.0, 0.08)
    }

    fn lux_to_counts(lux: f64) -> u16 {
        (lux / RESOLUTION_LUX_PER_COUNT).round().clamp(0.0, 65535.0) as u16
    }

    /// Latch the word a read of the current pointer returns. Advances the light
    /// ramp exactly once per read transaction (only for the ALS channel).
    fn latch_read_word(&mut self) {
        self.read_word = match self.pointer {
            REG_ALS => Self::lux_to_counts(self.lux.advance()),
            // White channel runs a bit brighter than ALS; don't advance again.
            REG_WHITE => Self::lux_to_counts(self.lux.value() * 1.15),
            r if (r as usize) < self.conf.len() => self.conf[r as usize],
            _ => 0,
        };
        self.latched = true;
    }
}

impl I2cDevice for Veml7700 {
    fn address(&self) -> u8 {
        self.address
    }

    fn start(&mut self) {
        // (Re)START frames either a write phase (pointer set) or the read phase.
        // Rewind the read cursor and clear the latch so the next read advances
        // the ramp once; keep the pointer set by the preceding write.
        self.write_buf.clear();
        self.read_byte_idx = 0;
        self.latched = false;
    }

    fn stop(&mut self) {
        // The C3 controller only calls start() on a repeated START, so clear the
        // command accumulator at transaction end too — otherwise a config-write
        // transaction leaves stale bytes that corrupt the next register pointer.
        self.write_buf.clear();
    }

    fn write(&mut self, data: u8) {
        self.write_buf.push(data);
        match self.write_buf.len() {
            1 => self.pointer = data,
            // 16-bit LE config write: low byte then high byte.
            3 => {
                if (self.pointer as usize) < self.conf.len() {
                    let lo = self.write_buf[1] as u16;
                    let hi = self.write_buf[2] as u16;
                    self.conf[self.pointer as usize] = (hi << 8) | lo;
                }
            }
            _ => {}
        }
    }

    fn read(&mut self) -> u8 {
        // Latch the value on the first byte of the read, then stream it
        // little-endian: low byte first, then high byte.
        if !self.latched {
            self.latch_read_word();
        }
        let byte = match self.read_byte_idx {
            0 => (self.read_word & 0xFF) as u8,
            1 => (self.read_word >> 8) as u8,
            _ => 0xFF,
        };
        self.read_byte_idx += 1;
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

    /// Point at a register and read its 16-bit LE word.
    fn read_reg(d: &mut Veml7700, reg: u8) -> u16 {
        d.start();
        d.write(reg);
        d.start(); // repeated START into the read phase
        let lo = d.read() as u16;
        let hi = d.read() as u16;
        (hi << 8) | lo
    }

    #[test]
    fn address_defaults_to_0x10() {
        assert_eq!(Veml7700::new_default(0).address(), 0x10);
    }

    #[test]
    fn als_reads_back_as_plausible_lux() {
        let mut d = Veml7700::new_default(VEML7700_ADDR);
        let counts = read_reg(&mut d, REG_ALS);
        let lux = counts as f64 * RESOLUTION_LUX_PER_COUNT;
        assert!(
            (300.0..600.0).contains(&lux),
            "first read is bright-ish: {lux:.0}"
        );
    }

    #[test]
    fn light_dims_over_reads() {
        let mut d = Veml7700::new_default(VEML7700_ADDR);
        let mut first = 0.0;
        let mut last = 0.0;
        for i in 0..60 {
            let counts = read_reg(&mut d, REG_ALS);
            let lux = counts as f64 * RESOLUTION_LUX_PER_COUNT;
            if i == 0 {
                first = lux;
            }
            last = lux;
        }
        assert!(last < first, "room dims: {first:.0} -> {last:.0} lux");
        assert!(last < 150.0, "settles toward 90 lux: {last:.0}");
    }

    #[test]
    fn config_write_is_read_back() {
        let mut d = Veml7700::new_default(VEML7700_ADDR);
        // Write ALS_CONF = 0x1234 (LE: 0x34, 0x12).
        d.start();
        d.write(0x00);
        d.write(0x34);
        d.write(0x12);
        let v = read_reg(&mut d, 0x00);
        assert_eq!(v, 0x1234, "config register round-trips");
    }
}
