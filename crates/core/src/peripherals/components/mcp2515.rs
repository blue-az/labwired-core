// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.

//! MCP2515 CAN controller SPI register shell — no CAN bus frames required.
//!
//! Supports RESET, READ, WRITE, READ_STATUS, RX_STATUS, BIT_MODIFY for the
//! register file used by common Arduino MCP_CAN libraries.

use crate::peripherals::spi::SpiDevice;
use std::any::Any;
use std::sync::mpsc::{Receiver, Sender};

const INST_WRITE: u8 = 0x02;
const INST_READ: u8 = 0x03;
const INST_BITMOD: u8 = 0x05;
const INST_READ_STATUS: u8 = 0xA0;
const INST_RX_STATUS: u8 = 0xB0;
const INST_RESET: u8 = 0xC0;

const REG_CANSTAT: u8 = 0x0E;
const REG_CANCTRL: u8 = 0x0F;
const REG_CNF3: u8 = 0x28;
const REG_CNF2: u8 = 0x29;
const REG_CNF1: u8 = 0x2A;
const REG_CANINTF: u8 = 0x2C;
#[cfg(test)]
const REG_EFLG: u8 = 0x2D;
const REG_TXB0CTRL: u8 = 0x30;
const REG_TXB0SIDH: u8 = 0x31;
#[cfg(test)]
const REG_TXB0D0: u8 = 0x36;
const REG_TXB1CTRL: u8 = 0x40;
const REG_TXB1SIDH: u8 = 0x41;
#[cfg(test)]
const REG_TXB1D0: u8 = 0x46;
const REG_TXB2CTRL: u8 = 0x50;
const REG_TXB2SIDH: u8 = 0x51;
#[cfg(test)]
const REG_TXB2D0: u8 = 0x56;
const REG_RXB0SIDH: u8 = 0x61;
const REG_RXB1SIDH: u8 = 0x71;

const TXREQ: u8 = 0x08;
#[cfg(test)]
const CANINTF_RX0IF: u8 = 0x01;
#[cfg(test)]
const CANINTF_RX1IF: u8 = 0x02;
const CANINTF_TX0IF: u8 = 0x04;
const CANINTF_MERRF: u8 = 0x80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TxBuffer {
    id: u32,
    dlc: u8,
    data: [u8; 8],
    pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RxBuffer {
    id: u32,
    dlc: u8,
    data: [u8; 8],
    full: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpMode {
    Normal,
    Sleep,
    Loopback,
    ListenOnly,
    Config,
}

impl OpMode {
    fn bits(self) -> u8 {
        match self {
            Self::Normal => 0x00,
            Self::Sleep => 0x20,
            Self::Loopback => 0x40,
            Self::ListenOnly => 0x60,
            Self::Config => 0x80,
        }
    }

    fn from_request(bits: u8) -> Option<Self> {
        match bits & 0xE0 {
            0x00 => Some(Self::Normal),
            0x20 => Some(Self::Sleep),
            0x40 => Some(Self::Loopback),
            0x60 => Some(Self::ListenOnly),
            0x80 => Some(Self::Config),
            _ => None,
        }
    }
}

#[allow(dead_code)] // Task 3 will use this when placing bus frames into RX registers.
fn encode_standard_id(id: u32) -> Option<[u8; 4]> {
    (id <= 0x7FF).then_some([(id >> 3) as u8, ((id & 7) << 5) as u8, 0, 0])
}

fn decode_standard_id(bytes: [u8; 4]) -> Option<u32> {
    if bytes[1] & 0x08 != 0 || bytes[2] != 0 || bytes[3] != 0 {
        return None;
    }
    Some(((bytes[0] as u32) << 3) | ((bytes[1] as u32) >> 5))
}

pub struct Mcp2515 {
    cs_pin: String,
    regs: [u8; 128],
    phase: Phase,
    inst: u8,
    addr: u8,
    bitmod_mask: u8,
    component_id: Option<String>,
    bus_tx: Option<Sender<crate::network::CanFrame>>,
    bus_rx: Option<Receiver<crate::network::CanFrame>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Instruction,
    Address,
    Data,
    BitModMask,
    BitModData,
    Status,
    Ignore,
}

impl Mcp2515 {
    pub fn new(cs_pin: impl Into<String>) -> Self {
        let mut regs = [0u8; 128];
        // Power-on: configuration mode (REQOP = 100)
        regs[REG_CANCTRL as usize] = 0x87;
        regs[REG_CANSTAT as usize] = 0x80;
        Self {
            cs_pin: cs_pin.into(),
            regs,
            phase: Phase::Instruction,
            inst: 0,
            addr: 0,
            bitmod_mask: 0,
            component_id: None,
            bus_tx: None,
            bus_rx: None,
        }
    }

    fn reset(&mut self) {
        self.regs = [0; 128];
        self.regs[REG_CANCTRL as usize] = 0x87;
        self.regs[REG_CANSTAT as usize] = OpMode::Config.bits();
    }

    fn timing_is_500k(&self) -> bool {
        let cnf1 = self.regs[REG_CNF1 as usize];
        let cnf2 = self.regs[REG_CNF2 as usize];
        let cnf3 = self.regs[REG_CNF3 as usize];
        if cnf2 & 0x80 == 0 {
            return false;
        }
        let brp = u32::from(cnf1 & 0x3F) + 1;
        let prop = u32::from(cnf2 & 0x07) + 1;
        let phase1 = u32::from((cnf2 >> 3) & 0x07) + 1;
        let phase2 = u32::from(cnf3 & 0x07) + 1;
        let tq = 1 + prop + phase1 + phase2;
        let bitrate = 16_000_000 / (2 * brp * tq);
        bitrate.abs_diff(500_000) <= 5_000
    }

    fn write_register(&mut self, address: u8, value: u8) {
        let address = address & 0x7F;
        if address == REG_CANSTAT {
            return;
        }
        self.regs[address as usize] = value;
        if address != REG_CANCTRL {
            return;
        }
        let requested = OpMode::from_request(value);
        let accepted = requested
            .filter(|mode| matches!(mode, OpMode::Config | OpMode::Sleep) || self.timing_is_500k());
        if let Some(mode) = accepted {
            self.regs[REG_CANSTAT as usize] =
                (self.regs[REG_CANSTAT as usize] & 0x1F) | mode.bits();
            self.regs[REG_CANINTF as usize] &= !CANINTF_MERRF;
        } else {
            self.regs[REG_CANINTF as usize] |= CANINTF_MERRF;
        }
    }

    fn tx_buffer(&self, index: usize) -> TxBuffer {
        let ctrl = [REG_TXB0CTRL, REG_TXB1CTRL, REG_TXB2CTRL][index];
        let sidh = ctrl + 1;
        let id = decode_standard_id([
            self.regs[sidh as usize],
            self.regs[sidh as usize + 1],
            self.regs[sidh as usize + 2],
            self.regs[sidh as usize + 3],
        ])
        .unwrap_or(0);
        let dlc = self.regs[sidh as usize + 4] & 0x0F;
        let mut data = [0; 8];
        data.copy_from_slice(&self.regs[sidh as usize + 5..sidh as usize + 13]);
        TxBuffer {
            id,
            dlc,
            data,
            pending: self.regs[ctrl as usize] & TXREQ != 0,
        }
    }

    fn rx_buffer(&self, index: usize) -> RxBuffer {
        let sidh = [REG_RXB0SIDH, REG_RXB1SIDH][index];
        let id = decode_standard_id([
            self.regs[sidh as usize],
            self.regs[sidh as usize + 1],
            self.regs[sidh as usize + 2],
            self.regs[sidh as usize + 3],
        ])
        .unwrap_or(0);
        let dlc = self.regs[sidh as usize + 4] & 0x0F;
        let mut data = [0; 8];
        data.copy_from_slice(&self.regs[sidh as usize + 5..sidh as usize + 13]);
        RxBuffer {
            id,
            dlc,
            data,
            full: self.regs[REG_CANINTF as usize] & (1 << index) != 0,
        }
    }

    fn read_status(&self) -> u8 {
        let intf = self.regs[REG_CANINTF as usize];
        let mut status = intf & 0x03;
        for index in 0..3 {
            let buffer = self.tx_buffer(index);
            if buffer.pending {
                status |= 1 << (2 + index * 2);
            }
            if intf & (CANINTF_TX0IF << index) != 0 {
                status |= 1 << (3 + index * 2);
            }
        }
        status
    }

    fn rx_status(&self) -> u8 {
        let rx0 = self.rx_buffer(0);
        let rx1 = self.rx_buffer(1);
        let full = (if rx0.full { 0x40 } else { 0 }) | (if rx1.full { 0x80 } else { 0 });
        let selected = if rx0.full {
            Some(REG_RXB0SIDH)
        } else if rx1.full {
            Some(REG_RXB1SIDH)
        } else {
            None
        };
        let Some(sidh) = selected else {
            return 0;
        };
        let filter_hit = self.regs[sidh as usize - 1] & 0x07;
        let standard_remote = if self.regs[sidh as usize + 4] & 0x40 != 0 {
            0x08
        } else {
            0
        };
        full | standard_remote | filter_hit
    }
}

impl SpiDevice for Mcp2515 {
    fn needs_external_bus_poll(&self) -> bool {
        self.bus_rx.is_some()
    }

    fn component_id(&self) -> Option<&str> {
        self.component_id.as_deref()
    }

    fn attach_can_bus(
        &mut self,
        tx: Sender<crate::network::CanFrame>,
        rx: Receiver<crate::network::CanFrame>,
    ) -> anyhow::Result<()> {
        if self.bus_tx.is_some() || self.bus_rx.is_some() {
            anyhow::bail!("MCP2515 is already attached to a CAN bus");
        }
        self.bus_tx = Some(tx);
        self.bus_rx = Some(rx);
        Ok(())
    }
    fn cs_pin(&self) -> &str {
        &self.cs_pin
    }

    fn cs_select(&mut self) {
        self.phase = Phase::Instruction;
    }

    fn cs_release(&mut self) {
        if (0x90..=0x97).contains(&self.inst) {
            let buffer = (self.inst - 0x90) / 4;
            let intf = self.regs[REG_CANINTF as usize] & !(1 << buffer);
            self.write_register(REG_CANINTF, intf);
        }
        self.phase = Phase::Instruction;
    }

    fn transfer(&mut self, mosi: u8) -> u8 {
        match self.phase {
            Phase::Instruction => {
                self.inst = mosi;
                match mosi {
                    INST_RESET => {
                        self.reset();
                        self.phase = Phase::Ignore;
                        0
                    }
                    INST_READ | INST_WRITE => {
                        self.phase = Phase::Address;
                        0
                    }
                    INST_BITMOD => {
                        self.phase = Phase::Address;
                        0
                    }
                    INST_READ_STATUS => {
                        self.phase = Phase::Status;
                        0
                    }
                    INST_RX_STATUS => {
                        self.phase = Phase::Status;
                        0
                    }
                    0x81 | 0x82 | 0x84 => {
                        let index = mosi.trailing_zeros() as usize;
                        let ctrl = [REG_TXB0CTRL, REG_TXB1CTRL, REG_TXB2CTRL][index];
                        self.write_register(ctrl, self.regs[ctrl as usize] | TXREQ);
                        self.phase = Phase::Ignore;
                        0
                    }
                    0x40..=0x45 => {
                        let index = ((mosi - 0x40) / 2) as usize;
                        self.addr = [REG_TXB0SIDH, REG_TXB1SIDH, REG_TXB2SIDH][index]
                            + if mosi & 1 != 0 { 5 } else { 0 };
                        self.phase = Phase::Data;
                        0
                    }
                    0x90..=0x97 => {
                        let index = ((mosi - 0x90) / 4) as usize;
                        self.addr =
                            [REG_RXB0SIDH, REG_RXB1SIDH][index] + if mosi & 2 != 0 { 5 } else { 0 };
                        self.phase = Phase::Data;
                        0
                    }
                    _ => {
                        self.phase = Phase::Ignore;
                        0
                    }
                }
            }
            Phase::Address => {
                self.addr = mosi;
                self.phase = if self.inst == INST_BITMOD {
                    Phase::BitModMask
                } else {
                    Phase::Data
                };
                0
            }
            Phase::BitModMask => {
                self.bitmod_mask = mosi;
                self.phase = Phase::BitModData;
                0
            }
            Phase::BitModData => {
                let idx = self.addr as usize % self.regs.len();
                let cur = self.regs[idx];
                self.write_register(
                    self.addr,
                    (cur & !self.bitmod_mask) | (mosi & self.bitmod_mask),
                );
                self.phase = Phase::Ignore;
                0
            }
            Phase::Data => {
                let idx = self.addr as usize % self.regs.len();
                let miso = if self.inst == INST_READ || (0x90..=0x97).contains(&self.inst) {
                    self.regs[idx]
                } else {
                    0
                };
                if self.inst == INST_WRITE || (0x40..=0x45).contains(&self.inst) {
                    self.write_register(self.addr, mosi);
                }
                self.addr = self.addr.wrapping_add(1);
                miso
            }
            Phase::Status => {
                if self.inst == INST_READ_STATUS {
                    self.read_status()
                } else {
                    self.rx_status()
                }
            }
            Phase::Ignore => 0,
        }
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

use crate::peripherals::kit::{
    AttachCtx, Category, ConfigKey, ConfigType, KitMetadata, PeripheralKit, Transport,
};

pub struct Mcp2515Kit;
pub static MCP2515_KIT: Mcp2515Kit = Mcp2515Kit;

static MCP2515_METADATA: KitMetadata = KitMetadata {
    inputs: &[],
    device_type: "mcp2515",
    label: "MCP2515 CAN",
    summary: "SPI CAN controller register shell (no bus frames).",
    detail: "Microchip MCP2515 RESET/READ/WRITE/BIT_MODIFY for CANCTRL/CANSTAT and \
             friends. CAN wire protocol is not simulated in this thin shell.",
    transport: Transport::Spi,
    category: Category::Spi,
    config_keys: &[ConfigKey {
        name: "cs_pin",
        ty: ConfigType::Str,
        doc: "Chip-select GPIO pin (e.g. \"PA4\"). Defaults to PA4.",
    }],
    labs: &[],
};

impl PeripheralKit for Mcp2515Kit {
    fn metadata(&self) -> &'static KitMetadata {
        &MCP2515_METADATA
    }
    fn attach(&self, ctx: &mut AttachCtx<'_>) -> anyhow::Result<()> {
        let cs = ctx.config_str("cs_pin").unwrap_or("PA4").to_string();
        let mut dev = Mcp2515::new(cs);
        dev.component_id = Some(ctx.device_id().to_string());
        ctx.attach_spi_device(Box::new(dev))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transaction(dev: &mut Mcp2515, bytes: &[u8]) -> Vec<u8> {
        dev.cs_select();
        let result = bytes.iter().map(|byte| dev.transfer(*byte)).collect();
        dev.cs_release();
        result
    }

    fn read(dev: &mut Mcp2515, address: u8, count: usize) -> Vec<u8> {
        transaction(
            dev,
            &[INST_READ, address]
                .into_iter()
                .chain(std::iter::repeat_n(0, count))
                .collect::<Vec<_>>(),
        )[2..]
            .to_vec()
    }

    fn write(dev: &mut Mcp2515, address: u8, values: &[u8]) {
        transaction(
            dev,
            &[INST_WRITE, address]
                .into_iter()
                .chain(values.iter().copied())
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn external_poll_demand_starts_only_after_can_attachment() {
        let mut dev = Mcp2515::new("PA4");
        assert!(!dev.needs_external_bus_poll());
        let (tx, _outbound) = std::sync::mpsc::channel();
        let (_inbound, rx) = std::sync::mpsc::channel();
        dev.attach_can_bus(tx, rx).unwrap();
        assert!(dev.needs_external_bus_poll());
    }

    #[test]
    fn reset_and_read_canctrl() {
        let mut dev = Mcp2515::new("PA4");
        dev.cs_select();
        dev.transfer(INST_RESET);
        dev.cs_release();
        dev.cs_select();
        dev.transfer(INST_READ);
        dev.transfer(REG_CANCTRL);
        let v = dev.transfer(0x00);
        assert_eq!(v, 0x87);
        assert_eq!(read(&mut dev, REG_CANSTAT, 2), [0x80, 0x87]);
        assert_eq!(read(&mut dev, REG_CANINTF, 1), [0]);
        assert_eq!(read(&mut dev, REG_EFLG, 1), [0]);
    }

    #[test]
    fn write_canctrl_updates_canstat_opmode() {
        let mut dev = Mcp2515::new("PA4");
        write(&mut dev, REG_CNF3, &[0x01, 0xBC, 0x00]);
        dev.cs_select();
        dev.transfer(INST_WRITE);
        dev.transfer(REG_CANCTRL);
        dev.transfer(0x00); // normal mode
        dev.cs_release();
        dev.cs_select();
        dev.transfer(INST_READ);
        dev.transfer(REG_CANSTAT);
        let st = dev.transfer(0);
        assert_eq!(st & 0xE0, 0x00);
    }

    #[test]
    fn sequential_write_read_and_bit_modify_share_register_side_effects() {
        let mut dev = Mcp2515::new("PA4");
        write(&mut dev, REG_CNF3, &[0x01, 0xBC, 0x00]);
        assert_eq!(read(&mut dev, REG_CNF3, 3), [0x01, 0xBC, 0x00]);
        transaction(&mut dev, &[INST_BITMOD, REG_CANCTRL, 0xE0, 0x40]);
        assert_eq!(read(&mut dev, REG_CANCTRL, 1)[0] & 0xE0, 0x40);
        assert_eq!(read(&mut dev, REG_CANSTAT, 1)[0] & 0xE0, 0x40);
        for mode in [0x20, 0x60, 0x80] {
            write(&mut dev, REG_CANCTRL, &[mode]);
            assert_eq!(read(&mut dev, REG_CANSTAT, 1)[0] & 0xE0, mode);
        }
    }

    #[test]
    fn invalid_timing_rejects_active_mode_and_reports_configuration_error() {
        let mut dev = Mcp2515::new("PA4");
        write(&mut dev, REG_CANCTRL, &[0x00]);
        assert_eq!(read(&mut dev, REG_CANSTAT, 1)[0] & 0xE0, 0x80);
        assert_ne!(read(&mut dev, REG_CANINTF, 1)[0] & CANINTF_MERRF, 0);
    }

    #[test]
    fn load_tx_buffer_variants_and_rts_set_txreq_and_status() {
        let mut dev = Mcp2515::new("PA4");
        let frame = [0x24, 0x60, 0, 0, 3, 0x11, 0x22, 0x33];
        for (load, rts, ctrl, base) in [
            (0x40, 0x81, REG_TXB0CTRL, REG_TXB0SIDH),
            (0x42, 0x82, REG_TXB1CTRL, REG_TXB1SIDH),
            (0x44, 0x84, REG_TXB2CTRL, REG_TXB2SIDH),
        ] {
            transaction(
                &mut dev,
                &[load].into_iter().chain(frame).collect::<Vec<_>>(),
            );
            assert_eq!(read(&mut dev, base, frame.len()), frame);
            transaction(&mut dev, &[rts]);
            assert_ne!(read(&mut dev, ctrl, 1)[0] & TXREQ, 0);
        }
        assert_eq!(
            transaction(&mut dev, &[INST_READ_STATUS, 0])[1] & 0x54,
            0x54
        );

        transaction(&mut dev, &[0x41, 0xAA, 0xBB]);
        assert_eq!(read(&mut dev, REG_TXB0D0, 2), [0xAA, 0xBB]);
        transaction(&mut dev, &[0x43, 0xCC]);
        assert_eq!(read(&mut dev, REG_TXB1D0, 1), [0xCC]);
        transaction(&mut dev, &[0x45, 0xDD]);
        assert_eq!(read(&mut dev, REG_TXB2D0, 1), [0xDD]);
    }

    #[test]
    fn read_rx_buffer_variants_use_documented_header_and_data_offsets() {
        let mut dev = Mcp2515::new("PA4");
        let header0 = [0x24, 0x60, 0, 0, 2, 0xDE, 0xAD];
        let header1 = [0x64, 0x20, 0, 0, 2, 0xBE, 0xEF];
        write(&mut dev, REG_RXB0SIDH, &header0);
        write(&mut dev, REG_RXB1SIDH, &header1);
        for command in [0x90, 0x91] {
            assert_eq!(transaction(&mut dev, &[command, 0, 0])[1..], header0[..2]);
        }
        for command in [0x92, 0x93] {
            assert_eq!(transaction(&mut dev, &[command, 0, 0])[1..], [0xDE, 0xAD]);
        }
        for command in [0x94, 0x95] {
            assert_eq!(transaction(&mut dev, &[command, 0, 0])[1..], header1[..2]);
        }
        for command in [0x96, 0x97] {
            assert_eq!(transaction(&mut dev, &[command, 0, 0])[1..], [0xBE, 0xEF]);
        }

        write(&mut dev, REG_CANINTF, &[CANINTF_RX0IF | CANINTF_RX1IF]);
        transaction(&mut dev, &[0x90, 0]);
        assert_eq!(read(&mut dev, REG_CANINTF, 1), [CANINTF_RX1IF]);
        transaction(&mut dev, &[0x94, 0]);
        assert_eq!(read(&mut dev, REG_CANINTF, 1), [0]);
    }

    #[test]
    fn status_commands_reflect_interrupts_rx_full_and_standard_frame_kind() {
        let mut dev = Mcp2515::new("PA4");
        write(
            &mut dev,
            REG_CANINTF,
            &[CANINTF_RX0IF | CANINTF_RX1IF | CANINTF_TX0IF],
        );
        assert_eq!(transaction(&mut dev, &[INST_READ_STATUS, 0])[1], 0x0B);
        assert_eq!(transaction(&mut dev, &[INST_RX_STATUS, 0])[1] & 0xC0, 0xC0);

        write(&mut dev, 0x60, &[0x01]); // RXB0CTRL FILHIT0
        write(&mut dev, 0x65, &[0x40]); // RXB0DLC RTR
        assert_eq!(transaction(&mut dev, &[INST_RX_STATUS, 0])[1] & 0x0F, 0x09);
    }

    #[test]
    fn standard_identifier_helpers_round_trip_and_reject_extended_form() {
        for id in [0, 0x123, 0x7FF] {
            let encoded = encode_standard_id(id).unwrap();
            assert_eq!(decode_standard_id(encoded).unwrap(), id);
        }
        assert!(encode_standard_id(0x800).is_none());
        assert!(decode_standard_id([0, 0x08, 0, 0]).is_none());
    }

    #[test]
    fn chip_select_boundary_discards_partial_command_state() {
        let mut dev = Mcp2515::new("PA4");
        transaction(&mut dev, &[INST_WRITE, REG_CNF3]);
        transaction(&mut dev, &[0x01]);
        assert_eq!(read(&mut dev, REG_CNF3, 1), [0]);
    }
}
