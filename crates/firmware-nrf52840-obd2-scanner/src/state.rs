pub mod flags {
    pub const CONNECTED: u16 = 1 << 0;
    pub const STALE: u16 = 1 << 1;
    pub const DTC_PRESENT: u16 = 1 << 2;
    pub const TIMEOUT: u16 = 1 << 3;
    pub const MALFORMED: u16 = 1 << 4;
    pub const RX_OVERFLOW: u16 = 1 << 5;
    pub const CAN_CONFIG_ERROR: u16 = 1 << 6;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScannerState {
    pub rpm: u16,
    pub speed_kph: u8,
    pub coolant_c: i16,
    pub dtc_count: u8,
    pub status_flags: u16,
    pub generation: u32,
    pub sample_age: u16,
    pub vin_valid: bool,
    pub vin: [u8; 17],
}

impl Default for ScannerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ScannerState {
    pub const fn new() -> Self {
        Self {
            rpm: 0,
            speed_kph: 0,
            coolant_c: 0,
            dtc_count: 0,
            status_flags: flags::STALE,
            generation: 0,
            sample_age: 0,
            vin_valid: false,
            vin: [0; 17],
        }
    }

    pub const fn has(&self, mask: u16) -> bool {
        self.status_flags & mask != 0
    }

    pub fn mark_fresh(&mut self) {
        self.status_flags |= flags::CONNECTED;
        self.status_flags &= !(flags::STALE | flags::TIMEOUT);
        self.sample_age = 0;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn mark_timeout(&mut self) {
        self.status_flags |= flags::TIMEOUT | flags::STALE;
        self.status_flags &= !flags::CONNECTED;
    }

    pub fn increment_age(&mut self) {
        self.sample_age = self.sample_age.saturating_add(1);
    }

    pub fn update_dtc_count(&mut self, count: u8) {
        self.dtc_count = count;
        if count == 0 {
            self.status_flags &= !flags::DTC_PRESENT;
        } else {
            self.status_flags |= flags::DTC_PRESENT;
        }
    }

    pub fn set_vin(&mut self, vin: [u8; 17]) {
        self.vin = vin;
        self.vin_valid = true;
    }

    pub fn invalidate_vin(&mut self) {
        self.vin = [0; 17];
        self.vin_valid = false;
    }

    pub fn set_error(&mut self, error_flag: u16) {
        self.status_flags |=
            error_flag & (flags::MALFORMED | flags::RX_OVERFLOW | flags::CAN_CONFIG_ERROR);
    }
}
