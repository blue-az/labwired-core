#![no_std]
#![no_main]

use core::hint::spin_loop;
use cortex_m_rt::entry;
use panic_halt as _;

use firmware_nrf52840_obd2_scanner::{
    ble::{encode_manufacturer_payload, Radio},
    clear_dtcs_request, decode_clear_dtcs, decode_coolant, decode_dtcs, decode_rpm, decode_speed,
    decode_supported_pids, flags,
    mcp2515::{Error as CanError, Mcp2515},
    mode01_request, read_dtcs_request,
    ssd1306::{DisplayView, Ssd1306},
    vin_request, AcquisitionFailure, CanFrame, IsoTpEvent, ScannerState, VinReassembler,
};

const ECU_WAIT: u32 = 80_000;
const POLL_DELAY: u32 = 20_000;

#[no_mangle]
pub static mut SCANNER_RPM: u16 = 0;
#[no_mangle]
pub static mut SCANNER_SPEED_KPH: u8 = 0;
#[no_mangle]
pub static mut SCANNER_COOLANT_C: i16 = 0;
#[no_mangle]
pub static mut SCANNER_LIVE_VALID: u8 = 0;
#[no_mangle]
pub static mut SCANNER_DTC_COUNT: u8 = 0;
#[no_mangle]
pub static mut SCANNER_FLAGS: u16 = flags::STALE;
#[no_mangle]
pub static mut SCANNER_GENERATION: u32 = 0;
#[no_mangle]
pub static mut VIN_BYTES: [u8; 17] = [0; 17];
#[no_mangle]
pub static mut VIN_VALID: u8 = 0;
#[no_mangle]
pub static mut BLE_PAYLOAD: [u8; 9] = [0; 9];
#[no_mangle]
pub static mut CYCLE_COUNT: u32 = 0;
#[no_mangle]
pub static mut TX_DONE_COUNT: u32 = 0;
/// Write nonzero to request deterministic Mode 04 transmission next cycle.
#[no_mangle]
pub static mut CLEAR_DTC_REQUEST: u8 = 0;
/// 0=idle, 1=pending, 2=positive response, 3=timeout/malformed.
#[no_mangle]
pub static mut CLEAR_DTC_RESULT: u8 = 0;

#[entry]
fn main() -> ! {
    let mut can = Mcp2515::new();
    let mut oled = Ssd1306::new();
    let mut radio = Radio::new();
    let mut state = ScannerState::new();
    if can.init().is_err() {
        state.set_error(flags::CAN_CONFIG_ERROR);
    }
    if !oled.init() {
        state.set_error(flags::CAN_CONFIG_ERROR);
    }
    if !radio.init() {
        state.set_error(flags::CAN_CONFIG_ERROR);
    }

    // PID 00 is retried until discovery succeeds. Live/setup requests never precede it.
    let mut supported = None;
    let mut dtc_done = false;
    let mut vin_done = false;
    let mut live_slot = 0u8;
    loop {
        state.increment_age();
        unsafe {
            if CLEAR_DTC_REQUEST != 0 {
                CLEAR_DTC_REQUEST = 0;
                CLEAR_DTC_RESULT = 1;
                CLEAR_DTC_RESULT = match transact(&mut can, clear_dtcs_request()) {
                    Ok(frame) if decode_clear_dtcs(&frame).is_ok() => 2,
                    Ok(_) => {
                        state.apply_failure(AcquisitionFailure::Malformed);
                        3
                    }
                    Err(failure) => {
                        state.apply_failure(failure);
                        3
                    }
                };
            }
        }

        if supported.is_none() {
            match transact(&mut can, mode01_request(0)) {
                Ok(frame) => match decode_supported_pids(&frame) {
                    Ok(map) => {
                        supported = Some(map);
                        state.set_required_live(required_live_mask(map));
                    }
                    Err(_) => state.apply_failure(AcquisitionFailure::Malformed),
                },
                Err(failure) => state.apply_failure(failure),
            }
        }

        let pid = [0x0c, 0x0d, 0x05][live_slot as usize];
        live_slot = (live_slot + 1) % 3;
        let supported_pid = supported
            .map(|map| map & (1 << (32 - pid)) != 0)
            .unwrap_or(false);
        if supported_pid {
            match transact(&mut can, mode01_request(pid)) {
                Ok(frame) => {
                    let valid = match pid {
                        0x0c => decode_rpm(&frame).map(|v| state.record_rpm(v)),
                        0x0d => decode_speed(&frame).map(|v| state.record_speed(v)),
                        _ => decode_coolant(&frame).map(|v| state.record_coolant(v)),
                    };
                    if valid.is_err() {
                        state.apply_failure(AcquisitionFailure::Malformed);
                    }
                }
                Err(failure) => state.apply_failure(failure),
            }
        }

        if state.has_all(flags::CONNECTED) {
            if !dtc_done {
                match retrieve_dtcs(&mut can, &mut state) {
                    SetupResult::Done => dtc_done = true,
                    SetupResult::PermanentFailure => dtc_done = true,
                    SetupResult::Retry => {}
                }
            }
            if state.has_all(flags::CONNECTED) && !vin_done {
                match retrieve_vin(&mut can, &mut state) {
                    SetupResult::Done => vin_done = true,
                    SetupResult::PermanentFailure => vin_done = true,
                    SetupResult::Retry => {}
                }
            }
        }

        // Always sample IRQ and read CANINTF; labs need deterministic polling.
        let _irq_asserted = can.irq_asserted();
        match can.interrupt_flags() {
            Ok(_interrupts) => match can.read(0x1d) {
                Ok(eflg) if eflg & 0xc0 != 0 => {
                    state.apply_failure(AcquisitionFailure::Overflow);
                    if can.clear_overflow().is_err() {
                        state.apply_failure(AcquisitionFailure::Configuration);
                    }
                }
                Ok(_) => {}
                Err(error) => state.apply_failure(driver_failure(error)),
            },
            Err(error) => state.apply_failure(driver_failure(error)),
        }
        publish(&state);
        let payload = encode_manufacturer_payload(&state);
        unsafe {
            BLE_PAYLOAD = payload;
        }
        if radio.transmit(&payload) {
            unsafe {
                TX_DONE_COUNT = TX_DONE_COUNT.wrapping_add(1);
            }
        }
        let view = DisplayView::from_state(&state);
        oled.render(&view);
        let _ = oled.update();
        unsafe {
            CYCLE_COUNT = CYCLE_COUNT.wrapping_add(1);
        }
        for _ in 0..POLL_DELAY {
            spin_loop();
        }
    }
}

fn transact(can: &mut Mcp2515, request: CanFrame) -> Result<CanFrame, AcquisitionFailure> {
    can.send(&request).map_err(driver_failure)?;
    for _ in 0..ECU_WAIT {
        match can.receive() {
            Ok(frame) => return Ok(frame),
            Err(CanError::NoFrame) => spin_loop(),
            Err(error) => return Err(driver_failure(error)),
        }
    }
    Err(AcquisitionFailure::Timeout)
}

enum SetupResult {
    Done,
    Retry,
    PermanentFailure,
}

fn retrieve_dtcs(can: &mut Mcp2515, state: &mut ScannerState) -> SetupResult {
    match transact(can, read_dtcs_request()) {
        Ok(frame) => match decode_dtcs(&frame) {
            Ok(dtcs) => {
                state.update_dtc_count(dtcs.count);
                SetupResult::Done
            }
            Err(_) => {
                state.apply_failure(AcquisitionFailure::Malformed);
                SetupResult::PermanentFailure
            }
        },
        Err(failure) => {
            state.apply_failure(failure);
            retry_kind(failure)
        }
    }
}

fn retrieve_vin(can: &mut Mcp2515, state: &mut ScannerState) -> SetupResult {
    if let Err(error) = can.send(&vin_request()) {
        let failure = driver_failure(error);
        state.apply_failure(failure);
        return retry_kind(failure);
    }
    let mut reassembler = VinReassembler::new();
    for _ in 0..ECU_WAIT {
        match can.receive() {
            Ok(frame) => match reassembler.push(&frame) {
                Ok(IsoTpEvent::FlowControl(fc)) => {
                    if let Err(error) = can.send(&fc) {
                        let failure = driver_failure(error);
                        state.apply_failure(failure);
                        return retry_kind(failure);
                    }
                }
                Ok(IsoTpEvent::Complete(vin)) => {
                    state.set_vin(vin);
                    return SetupResult::Done;
                }
                Ok(IsoTpEvent::Pending) => {}
                Err(_) => {
                    state.apply_failure(AcquisitionFailure::Malformed);
                    return SetupResult::PermanentFailure;
                }
            },
            Err(CanError::NoFrame) => spin_loop(),
            Err(error) => {
                let failure = driver_failure(error);
                state.apply_failure(failure);
                return retry_kind(failure);
            }
        }
    }
    match reassembler.timeout() {
        Ok(()) => {
            state.apply_failure(AcquisitionFailure::Timeout);
            SetupResult::Retry
        }
        Err(_) => {
            state.apply_failure(AcquisitionFailure::Malformed);
            SetupResult::PermanentFailure
        }
    }
}

fn retry_kind(failure: AcquisitionFailure) -> SetupResult {
    if failure == AcquisitionFailure::Timeout {
        SetupResult::Retry
    } else {
        SetupResult::PermanentFailure
    }
}

fn driver_failure(error: CanError) -> AcquisitionFailure {
    match error {
        CanError::Timeout | CanError::NoFrame => AcquisitionFailure::Timeout,
        CanError::Overflow => AcquisitionFailure::Overflow,
        CanError::Configuration => AcquisitionFailure::Configuration,
    }
}

fn required_live_mask(map: u32) -> u8 {
    let mut mask = 0;
    if map & (1 << (32 - 0x0c)) != 0 {
        mask |= firmware_nrf52840_obd2_scanner::live::RPM;
    }
    if map & (1 << (32 - 0x0d)) != 0 {
        mask |= firmware_nrf52840_obd2_scanner::live::SPEED;
    }
    if map & (1 << (32 - 0x05)) != 0 {
        mask |= firmware_nrf52840_obd2_scanner::live::COOLANT;
    }
    mask
}

fn publish(state: &ScannerState) {
    unsafe {
        SCANNER_RPM = state.rpm;
        SCANNER_SPEED_KPH = state.speed_kph;
        SCANNER_COOLANT_C = state.coolant_c;
        SCANNER_LIVE_VALID = state.live_valid;
        SCANNER_DTC_COUNT = state.dtc_count;
        SCANNER_FLAGS = state.status_flags;
        SCANNER_GENERATION = state.generation;
        VIN_BYTES = state.vin;
        VIN_VALID = state.vin_valid as u8;
    }
}
