//! Fixed BLE telemetry encoding and the nRF52840 raw RADIO transmitter.
//!
//! This uses the repository's Air Tracer contract: a BLE-1M-shaped raw packet,
//! not a complete standards-compliant GAP advertising PDU.

use core::ptr::{read_volatile, write_volatile};

use crate::{flags, ScannerState};

pub const PAYLOAD_LEN: usize = 9;
pub const VERSION: u8 = 1;
/// All currently meaningful state flags have stable, identical on-wire bits.
pub const WIRE_FLAGS: u16 = flags::CONNECTED
    | flags::STALE
    | flags::DTC_PRESENT
    | flags::TIMEOUT
    | flags::MALFORMED
    | flags::RX_OVERFLOW
    | flags::CAN_CONFIG_ERROR;

/// Layout: version, flags, RPM LE, speed, coolant+40, DTC count, generation LE16.
/// Coolant values below -40 C encode as 0 and above 215 C encode as 255.
pub fn encode_manufacturer_payload(state: &ScannerState) -> [u8; PAYLOAD_LEN] {
    debug_assert_eq!(WIRE_FLAGS & !0xff, 0);
    let coolant = state.coolant_c.saturating_add(40).clamp(0, 255) as u8;
    let rpm = state.rpm.to_le_bytes();
    let generation = (state.generation as u16).to_le_bytes();
    [
        VERSION,
        (state.status_flags & WIRE_FLAGS) as u8,
        rpm[0],
        rpm[1],
        state.speed_kph,
        coolant,
        state.dtc_count,
        generation[0],
        generation[1],
    ]
}

const CLOCK: usize = 0x4000_0000;
const RADIO: usize = 0x4000_1000;
const WAIT_LIMIT: u32 = 200_000;

pub struct Radio {
    packet: [u8; PAYLOAD_LEN + 2],
}

impl Default for Radio {
    fn default() -> Self {
        Self::new()
    }
}

impl Radio {
    pub const fn new() -> Self {
        Self {
            packet: [0; PAYLOAD_LEN + 2],
        }
    }

    pub fn init(&mut self) -> bool {
        unsafe {
            wr(CLOCK, 0x100, 0);
            wr(CLOCK, 0x000, 1);
            if !wait_set(CLOCK, 0x100) {
                return false;
            }
            self.packet[0] = 0xab;
            self.packet[1] = PAYLOAD_LEN as u8;
            wr(RADIO, 0x510, 3);
            wr(RADIO, 0x508, 42);
            wr(RADIO, 0x514, 8 | (1 << 8));
            wr(RADIO, 0x518, 0xff | (1 << 25));
            wr(RADIO, 0x51c, 0xcafe_ba00);
            wr(RADIO, 0x524, 0xbe);
            wr(RADIO, 0x52c, 0);
            wr(RADIO, 0x534, 3);
            wr(RADIO, 0x538, 0x065b);
            wr(RADIO, 0x53c, 0x55_5555);
            wr(RADIO, 0x554, 42);
            wr(RADIO, 0x504, self.packet.as_ptr() as u32);
        }
        true
    }

    pub fn transmit(&mut self, payload: &[u8; PAYLOAD_LEN]) -> bool {
        self.packet[2..].copy_from_slice(payload);
        unsafe {
            // Refresh pointer because this driver can be moved after init.
            wr(RADIO, 0x504, self.packet.as_ptr() as u32);
            wr(RADIO, 0x100, 0);
            wr(RADIO, 0x000, 1);
            if !wait_set(RADIO, 0x100) {
                return false;
            }
            wr(RADIO, 0x10c, 0);
            wr(RADIO, 0x008, 1);
            if !wait_set(RADIO, 0x10c) {
                return false;
            }
            wr(RADIO, 0x110, 0);
            wr(RADIO, 0x010, 1);
            wait_set(RADIO, 0x110)
        }
    }
}

unsafe fn wr(base: usize, offset: usize, value: u32) {
    write_volatile((base + offset) as *mut u32, value)
}
unsafe fn rd(base: usize, offset: usize) -> u32 {
    read_volatile((base + offset) as *const u32)
}
unsafe fn wait_set(base: usize, offset: usize) -> bool {
    for _ in 0..WAIT_LIMIT {
        if rd(base, offset) != 0 {
            return true;
        }
    }
    false
}
