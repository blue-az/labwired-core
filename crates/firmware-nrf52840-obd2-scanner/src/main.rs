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
    vin_request, CanFrame, IsoTpEvent, ScannerState, VinReassembler,
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

    // PID 00 is always queried first. Only supported live PIDs are subsequently polled.
    let supported =
        transact(&mut can, mode01_request(0)).and_then(|f| decode_supported_pids(&f).ok());
    let mut setup_done = false;
    let mut live_slot = 0u8;
    loop {
        state.increment_age();
        unsafe {
            if CLEAR_DTC_REQUEST != 0 {
                CLEAR_DTC_REQUEST = 0;
                CLEAR_DTC_RESULT = 1;
                CLEAR_DTC_RESULT = match transact(&mut can, clear_dtcs_request())
                    .and_then(|f| decode_clear_dtcs(&f).ok())
                {
                    Some(()) => 2,
                    None => 3,
                };
            }
        }

        if !setup_done && supported.is_some() {
            retrieve_dtcs(&mut can, &mut state);
            retrieve_vin(&mut can, &mut state);
            setup_done = true;
        }

        let pid = [0x0c, 0x0d, 0x05][live_slot as usize];
        live_slot = (live_slot + 1) % 3;
        let supported_pid = supported
            .map(|map| map & (1 << (32 - pid)) != 0)
            .unwrap_or(false);
        if supported_pid {
            match transact(&mut can, mode01_request(pid)) {
                Some(frame) => {
                    let valid = match pid {
                        0x0c => decode_rpm(&frame).map(|v| state.rpm = v),
                        0x0d => decode_speed(&frame).map(|v| state.speed_kph = v),
                        _ => decode_coolant(&frame).map(|v| state.coolant_c = v),
                    };
                    if valid.is_ok() {
                        state.mark_fresh();
                    } else {
                        state.set_error(flags::MALFORMED);
                    }
                }
                None => state.mark_timeout(),
            }
        } else if supported.is_none() {
            state.mark_timeout();
        }

        // Poll CANINTF even if physical IRQ is unwired; surface hardware overflow.
        if (can.irq_asserted() || can.interrupt_flags().unwrap_or(0) != 0)
            && can.read(0x1d).unwrap_or(0) & 0xc0 != 0
        {
            state.set_error(flags::RX_OVERFLOW);
            let _ = can.clear_overflow();
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

fn transact(can: &mut Mcp2515, request: CanFrame) -> Option<CanFrame> {
    if can.send(&request).is_err() {
        return None;
    }
    for _ in 0..ECU_WAIT {
        match can.receive() {
            Ok(frame) => return Some(frame),
            Err(CanError::NoFrame) => spin_loop(),
            Err(_) => return None,
        }
    }
    None
}

fn retrieve_dtcs(can: &mut Mcp2515, state: &mut ScannerState) {
    if let Some(frame) = transact(can, read_dtcs_request()) {
        match decode_dtcs(&frame) {
            Ok(dtcs) => state.update_dtc_count(dtcs.count),
            Err(_) => state.set_error(flags::MALFORMED),
        }
    }
}

fn retrieve_vin(can: &mut Mcp2515, state: &mut ScannerState) {
    if can.send(&vin_request()).is_err() {
        return;
    }
    let mut reassembler = VinReassembler::new();
    for _ in 0..ECU_WAIT {
        match can.receive() {
            Ok(frame) => match reassembler.push(&frame) {
                Ok(IsoTpEvent::FlowControl(fc)) => {
                    if can.send(&fc).is_err() {
                        return;
                    }
                }
                Ok(IsoTpEvent::Complete(vin)) => {
                    state.set_vin(vin);
                    return;
                }
                Ok(IsoTpEvent::Pending) => {}
                Err(_) => {
                    state.set_error(flags::MALFORMED);
                    return;
                }
            },
            Err(CanError::NoFrame) => spin_loop(),
            Err(_) => return,
        }
    }
    let _ = reassembler.timeout();
}

fn publish(state: &ScannerState) {
    unsafe {
        SCANNER_RPM = state.rpm;
        SCANNER_SPEED_KPH = state.speed_kph;
        SCANNER_COOLANT_C = state.coolant_c;
        SCANNER_DTC_COUNT = state.dtc_count;
        SCANNER_FLAGS = state.status_flags;
        SCANNER_GENERATION = state.generation;
        VIN_BYTES = state.vin;
        VIN_VALID = state.vin_valid as u8;
    }
}
