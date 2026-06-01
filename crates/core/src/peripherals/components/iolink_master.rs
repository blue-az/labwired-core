// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.

use crate::peripherals::uart::UartStreamDevice;
use std::any::Any;
use std::collections::VecDeque;

/// IO-Link 6-bit checksum (CRC6). Polynomial `0x1D << 2`, initial value `0x15`.
/// Ports `calculate_crc6` from the project's reference virtual-master crc.py.
pub(crate) fn crc6(data: &[u8]) -> u8 {
    let mut crc: u8 = 0x15;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ (0x1D << 2);
            } else {
                crc <<= 1;
            }
        }
    }
    (crc >> 2) & 0x3F
}

/// Encode a Type 0 master frame: `[MC, CK]` with `CK = crc6([MC, CKT=0x00])`.
pub(crate) fn encode_type0(mc: u8) -> Vec<u8> {
    vec![mc, crc6(&[mc, 0x00])]
}

/// Encode a Type 1 cyclic request: `[MC=0x00, CKT=0x00, PD_out..., OD=0x00, CK]`.
pub(crate) fn encode_type1_cycle(pd_out: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x00u8, 0x00];
    frame.extend_from_slice(pd_out);
    frame.push(0x00); // OD (1-byte, idle)
    let ck = crc6(&frame);
    frame.push(ck);
    frame
}

/// Parsed device OPERATE response.
#[derive(Debug, Clone)]
pub(crate) struct OperateResponse {
    pub(crate) pd: Vec<u8>,
    pub(crate) pd_valid: bool,
    pub(crate) checksum_ok: bool,
}

/// Decode `[status, PD_in..., OD..., CK]` (length `1 + pd_in_len + od_len + 1`).
pub(crate) fn decode_operate(data: &[u8], pd_in_len: usize, od_len: usize) -> OperateResponse {
    if data.len() < 2 + pd_in_len + od_len {
        return OperateResponse {
            pd: Vec::new(),
            pd_valid: false,
            checksum_ok: false,
        };
    }
    let status = data[0];
    let pd_end = data.len() - od_len - 1;
    let pd = data[1..pd_end].to_vec();
    let ck = data[data.len() - 1];
    let checksum_ok = crc6(&data[..data.len() - 1]) == ck;
    let pd_valid = status & 0x20 != 0;
    OperateResponse {
        pd,
        pd_valid,
        checksum_ok,
    }
}

/// IO-Link COM speed (display/config only in this model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IolinkComSpeed {
    Com1,
    Com2,
    Com3,
}

/// Link state exposed to the inspector panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IolinkLinkState {
    Startup,
    Operate,
}

/// Ticks the master waits (one `poll` per UART tick) between frames. The
/// simulated device executes far slower than the UART advances, so frames are
/// paced generously to guarantee the device has fully processed (and replied
/// to) one frame before the next arrives — this is what keeps the device's
/// byte framing aligned. Tunable; sized for the `-O0` demo firmware.
const FRAME_GAP_TICKS: u32 = 6000;

/// Number of IDLE frames sent before the OPERATE transition. The device needs
/// one valid frame to leave AWAITING_COMM for PREOPERATE; a few repeats absorb
/// any byte the wake-up detection consumed.
const IDLE_FRAMES: u32 = 4;

/// Native IO-Link master peer. Attaches to the firmware's UART as a
/// `UartStreamDevice`: `poll` drives the master's request bytes onto the firmware
/// RX path, `on_tx_byte` receives the device's response bytes from the firmware
/// TX path.
///
/// Drives a **deterministic, tick-paced** startup schedule rather than reacting
/// to response timing: wake-up (once) → several IDLE frames (→ PREOPERATE) → the
/// OPERATE transition (→ ESTAB_COM) → cyclic Type 1 requests (→ OPERATE). Process
/// data input is captured from the cyclic responses.
#[derive(Debug, serde::Serialize)]
pub struct IolinkMaster {
    pd_in_len: usize,
    od_len: usize,
    com: IolinkComSpeed,
    pub link_state: IolinkLinkState,
    /// Bytes still to send onto the firmware RX path (one frame at a time).
    #[serde(skip)]
    tx_queue: VecDeque<u8>,
    /// Device-response bytes accumulated since the current frame was queued.
    #[serde(skip)]
    rx_accum: Vec<u8>,
    /// Schedule position (0 = wake-up, then IDLEs, transition, cyclic Type 1).
    step: u32,
    /// UART ticks elapsed since the current frame finished sending.
    #[serde(skip)]
    gap_ticks: u32,
    /// Latest valid process-data input bytes received from the device.
    latest_pd: Vec<u8>,
    /// Latches true on the first valid cyclic frame and is intentionally sticky.
    pub pd_valid: bool,
}

impl IolinkMaster {
    pub fn new(pd_in_len: usize, od_len: usize, com: IolinkComSpeed) -> Self {
        let mut m = Self {
            pd_in_len,
            od_len,
            com,
            link_state: IolinkLinkState::Startup,
            tx_queue: VecDeque::new(),
            rx_accum: Vec::new(),
            step: 0,
            gap_ticks: 0,
            latest_pd: vec![0u8; pd_in_len.max(1)],
            pd_valid: false,
        };
        m.queue_next_frame(); // queue the wake-up immediately
        m
    }

    /// First process-data input byte (channel bitmap for a DI hub).
    pub fn input_byte(&self) -> u8 {
        self.latest_pd.first().copied().unwrap_or(0)
    }

    pub fn com_speed(&self) -> IolinkComSpeed {
        self.com
    }

    fn operate_response_len(&self) -> usize {
        1 + self.pd_in_len + self.od_len + 1
    }

    /// Queue the next frame in the startup/cyclic schedule and advance `step`.
    fn queue_next_frame(&mut self) {
        self.rx_accum.clear();
        let idle_end = 1 + IDLE_FRAMES; // steps [1..=IDLE_FRAMES] are IDLE
        if self.step == 0 {
            self.tx_queue.push_back(0x55); // wake-up pulse (once)
        } else if self.step < idle_end {
            for b in encode_type0(0x00) {
                self.tx_queue.push_back(b); // Type 0 IDLE → PREOPERATE
            }
        } else if self.step == idle_end {
            for b in encode_type0(0x0F) {
                self.tx_queue.push_back(b); // OPERATE transition (MC=0x0F) → ESTAB_COM
            }
        } else {
            for b in encode_type1_cycle(&[]) {
                self.tx_queue.push_back(b); // cyclic Type 1 → OPERATE + process data
            }
            self.link_state = IolinkLinkState::Operate;
        }
        // Hold `step` at the first cyclic index so it keeps repeating Type 1.
        if self.step <= idle_end {
            self.step += 1;
        }
    }
}

impl UartStreamDevice for IolinkMaster {
    fn poll(&mut self, _elapsed_us: u32) -> Option<u8> {
        if let Some(byte) = self.tx_queue.pop_front() {
            return Some(byte);
        }
        // Frame fully sent: wait the inter-frame gap, then queue the next one.
        self.gap_ticks = self.gap_ticks.saturating_add(1);
        if self.gap_ticks < FRAME_GAP_TICKS {
            return None;
        }
        self.gap_ticks = 0;
        self.queue_next_frame();
        self.tx_queue.pop_front()
    }

    fn on_tx_byte(&mut self, byte: u8) {
        // Accumulate the device's reply to the current frame. Once a cyclic
        // (OPERATE) response is complete, decode and latch the process data.
        if self.rx_accum.len() < 64 {
            self.rx_accum.push(byte);
        }
        if self.link_state == IolinkLinkState::Operate
            && self.rx_accum.len() >= self.operate_response_len()
        {
            let n = self.operate_response_len();
            let resp = decode_operate(&self.rx_accum[..n], self.pd_in_len, self.od_len);
            if resp.checksum_ok && resp.pd_valid {
                self.latest_pd = resp.pd;
                self.pd_valid = true;
            }
            self.rx_accum.clear();
        }
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pump ticks and return the bytes of exactly the next frame: skip any
    /// leading inter-frame gap, collect the frame's bytes, stop at the next gap.
    fn drain(m: &mut IolinkMaster) -> Vec<u8> {
        let mut out = Vec::new();
        let mut started = false;
        for _ in 0..(FRAME_GAP_TICKS * 2 + 16) {
            match m.poll(1000) {
                Some(b) => {
                    out.push(b);
                    started = true;
                }
                None => {
                    if started {
                        break;
                    }
                }
            }
        }
        out
    }

    #[test]
    fn crc6_matches_iolink_vectors() {
        assert_eq!(crc6(&[0x00, 0x00]), 0x24);
        assert_eq!(crc6(&[0x0F, 0x00]), 0x0D);
        assert_eq!(crc6(&[0x95, 0x00]), 0x1D);
        assert_eq!(crc6(&[0x20, 0xA5, 0x00]), 0x0D);
    }

    #[test]
    fn encodes_type0_idle_and_operate_transition() {
        assert_eq!(encode_type0(0x00), vec![0x00, 0x24]); // IDLE
        assert_eq!(encode_type0(0x0F), vec![0x0F, 0x0D]); // OPERATE transition
    }

    #[test]
    fn encodes_type1_di_cycle_with_no_output_pd() {
        assert_eq!(encode_type1_cycle(&[]), vec![0x00, 0x00, 0x00, 0x09]);
    }

    #[test]
    fn decodes_operate_response_and_extracts_pd() {
        let resp = decode_operate(&[0x20, 0xA5, 0x00, 0x0D], 1, 1);
        assert!(resp.checksum_ok);
        assert!(resp.pd_valid);
        assert_eq!(resp.pd, vec![0xA5]);
    }

    #[test]
    fn decode_operate_handles_two_byte_pd() {
        let mut frame = vec![0x20u8, 0xAA, 0xBB, 0x00];
        let ck = crc6(&frame);
        frame.push(ck);
        let resp = decode_operate(&frame, 2, 1);
        assert!(resp.checksum_ok);
        assert!(resp.pd_valid);
        assert_eq!(resp.pd, vec![0xAA, 0xBB]);
    }

    #[test]
    fn schedule_walks_wakeup_idle_transition_then_cyclic_type1() {
        let mut m = IolinkMaster::new(1, 1, IolinkComSpeed::Com2);

        // Step 0: wake-up pulse.
        assert_eq!(drain(&mut m), vec![0x55]);
        assert_eq!(m.link_state, IolinkLinkState::Startup);

        // Steps 1..=IDLE_FRAMES: IDLE frames (→ PREOPERATE on the device).
        for _ in 0..IDLE_FRAMES {
            assert_eq!(drain(&mut m), vec![0x00, 0x24]);
        }
        assert_eq!(m.link_state, IolinkLinkState::Startup);

        // Next: the OPERATE transition (MC=0x0F).
        assert_eq!(drain(&mut m), vec![0x0F, 0x0D]);

        // Then cyclic Type 1 requests, repeating forever.
        assert_eq!(drain(&mut m), vec![0x00, 0x00, 0x00, 0x09]);
        assert_eq!(m.link_state, IolinkLinkState::Operate);
        assert_eq!(drain(&mut m), vec![0x00, 0x00, 0x00, 0x09]);
    }

    #[test]
    fn captures_process_data_from_cyclic_response() {
        let mut m = IolinkMaster::new(1, 1, IolinkComSpeed::Com2);
        // Advance the schedule to the cyclic (OPERATE) phase.
        while m.link_state != IolinkLinkState::Operate {
            drain(&mut m);
        }
        // Device replies to the cyclic request with PD = 0xA5, valid.
        for b in [0x20u8, 0xA5, 0x00, 0x0D] {
            m.on_tx_byte(b);
        }
        assert_eq!(m.input_byte(), 0xA5);
        assert!(m.pd_valid);
    }
}
