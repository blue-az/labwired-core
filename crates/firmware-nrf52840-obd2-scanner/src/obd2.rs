pub const REQUEST_ID: u16 = 0x7df;
pub const FLOW_CONTROL_ID: u16 = 0x7e0;
pub const RESPONSE_ID: u16 = 0x7e8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CanFrame {
    pub id: u16,
    pub len: u8,
    pub data: [u8; 8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidLength,
    WrongId,
    Malformed,
    ShortPayload,
    NegativeResponse(u8),
    UnsupportedService,
    UnsupportedPid,
    Sequence,
    UnexpectedFrame,
    Oversize,
    Incomplete,
}

pub const fn mode01_request(pid: u8) -> CanFrame {
    request([2, 1, pid, 0, 0, 0, 0, 0])
}

pub const fn read_dtcs_request() -> CanFrame {
    request([1, 3, 0, 0, 0, 0, 0, 0])
}

pub const fn clear_dtcs_request() -> CanFrame {
    request([1, 4, 0, 0, 0, 0, 0, 0])
}

pub const fn vin_request() -> CanFrame {
    request([2, 9, 2, 0, 0, 0, 0, 0])
}

const fn request(data: [u8; 8]) -> CanFrame {
    CanFrame {
        id: REQUEST_ID,
        len: 8,
        data,
    }
}

fn single_frame_payload(frame: &CanFrame) -> Result<&[u8], Error> {
    if frame.len > 8 {
        return Err(Error::InvalidLength);
    }
    if frame.id != RESPONSE_ID {
        return Err(Error::WrongId);
    }
    if frame.len == 0 || frame.data[0] & 0xf0 != 0 {
        return Err(Error::Malformed);
    }
    let payload_len = usize::from(frame.data[0] & 0x0f);
    if payload_len == 0 || payload_len + 1 > usize::from(frame.len) {
        return Err(Error::ShortPayload);
    }
    Ok(&frame.data[1..=payload_len])
}

fn positive_pid_payload(
    frame: &CanFrame,
    service: u8,
    pid: u8,
    needed: usize,
) -> Result<&[u8], Error> {
    let payload = single_frame_payload(frame)?;
    check_negative(payload, service)?;
    if payload[0] != service.wrapping_add(0x40) {
        return Err(Error::UnsupportedService);
    }
    if payload.len() < 2 {
        return Err(Error::ShortPayload);
    }
    if payload[1] != pid {
        return Err(Error::UnsupportedPid);
    }
    if payload.len() < needed {
        return Err(Error::ShortPayload);
    }
    Ok(payload)
}

fn check_negative(payload: &[u8], requested_service: u8) -> Result<(), Error> {
    if payload[0] == 0x7f {
        if payload.len() < 3 {
            return Err(Error::ShortPayload);
        }
        if payload[1] != requested_service {
            return Err(Error::UnsupportedService);
        }
        return Err(Error::NegativeResponse(payload[2]));
    }
    Ok(())
}

pub fn decode_supported_pids(frame: &CanFrame) -> Result<u32, Error> {
    let p = positive_pid_payload(frame, 1, 0, 6)?;
    Ok(u32::from_be_bytes([p[2], p[3], p[4], p[5]]))
}

pub fn decode_rpm(frame: &CanFrame) -> Result<u16, Error> {
    let p = positive_pid_payload(frame, 1, 0x0c, 4)?;
    Ok((u16::from(p[2]) * 256 + u16::from(p[3])) / 4)
}

pub fn decode_speed(frame: &CanFrame) -> Result<u8, Error> {
    Ok(positive_pid_payload(frame, 1, 0x0d, 3)?[2])
}

pub fn decode_coolant(frame: &CanFrame) -> Result<i16, Error> {
    Ok(i16::from(positive_pid_payload(frame, 1, 5, 3)?[2]) - 40)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DtcSystem {
    Powertrain,
    Chassis,
    Body,
    Network,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dtc {
    pub system: DtcSystem,
    pub digits: [u8; 4],
}

impl Dtc {
    pub const fn ascii(self) -> [u8; 5] {
        let system = match self.system {
            DtcSystem::Powertrain => b'P',
            DtcSystem::Chassis => b'C',
            DtcSystem::Body => b'B',
            DtcSystem::Network => b'U',
        };
        [
            system,
            b'0' + self.digits[0],
            b'0' + self.digits[1],
            b'0' + self.digits[2],
            b'0' + self.digits[3],
        ]
    }

    const fn from_raw(raw: u16) -> Self {
        let system = match raw >> 14 {
            0 => DtcSystem::Powertrain,
            1 => DtcSystem::Chassis,
            2 => DtcSystem::Body,
            _ => DtcSystem::Network,
        };
        Self {
            system,
            digits: [
                ((raw >> 12) & 3) as u8,
                ((raw >> 8) & 0x0f) as u8,
                ((raw >> 4) & 0x0f) as u8,
                (raw & 0x0f) as u8,
            ],
        }
    }
}

const EMPTY_DTC: Dtc = Dtc {
    system: DtcSystem::Powertrain,
    digits: [0; 4],
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DtcList {
    pub dtcs: [Dtc; 3],
    pub count: u8,
}

pub fn decode_dtcs(frame: &CanFrame) -> Result<DtcList, Error> {
    let payload = single_frame_payload(frame)?;
    check_negative(payload, 3)?;
    if payload[0] != 0x43 {
        return Err(Error::UnsupportedService);
    }
    if (payload.len() - 1) % 2 != 0 {
        return Err(Error::Malformed);
    }
    let mut result = DtcList {
        dtcs: [EMPTY_DTC; 3],
        count: 0,
    };
    for pair in payload[1..].chunks_exact(2) {
        let raw = u16::from_be_bytes([pair[0], pair[1]]);
        if raw != 0 {
            result.dtcs[usize::from(result.count)] = Dtc::from_raw(raw);
            result.count += 1;
        }
    }
    Ok(result)
}

pub fn decode_clear_dtcs(frame: &CanFrame) -> Result<(), Error> {
    let payload = single_frame_payload(frame)?;
    check_negative(payload, 4)?;
    if payload[0] != 0x44 {
        return Err(Error::UnsupportedService);
    }
    if payload.len() != 1 {
        return Err(Error::Malformed);
    }
    Ok(())
}
