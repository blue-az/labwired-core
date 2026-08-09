#![cfg_attr(not(test), no_std)]

pub mod isotp;
pub mod obd2;
pub mod state;

pub use isotp::{IsoTpEvent, VinReassembler};
pub use obd2::{
    clear_dtcs_request, decode_clear_dtcs, decode_coolant, decode_dtcs, decode_rpm, decode_speed,
    decode_supported_pids, mode01_request, read_dtcs_request, vin_request, CanFrame, Dtc, DtcList,
    DtcSystem, Error, FLOW_CONTROL_ID, REQUEST_ID, RESPONSE_ID,
};
pub use state::{flags, ScannerState};

#[cfg(test)]
mod tests {
    use super::*;

    fn response(data: [u8; 8]) -> CanFrame {
        CanFrame {
            id: RESPONSE_ID,
            len: 8,
            data,
        }
    }

    #[test]
    fn exact_request_frames() {
        assert_eq!(mode01_request(0x0c).data, [2, 1, 0x0c, 0, 0, 0, 0, 0]);
        assert_eq!(read_dtcs_request().data, [1, 3, 0, 0, 0, 0, 0, 0]);
        assert_eq!(clear_dtcs_request().data, [1, 4, 0, 0, 0, 0, 0, 0]);
        assert_eq!(vin_request().data, [2, 9, 2, 0, 0, 0, 0, 0]);
        assert_eq!(mode01_request(0).id, REQUEST_ID);
    }

    #[test]
    fn mode01_decoders_and_supported_bitmap() {
        assert_eq!(
            decode_rpm(&response([4, 0x41, 0x0c, 0x2e, 0xe0, 0, 0, 0])),
            Ok(3000)
        );
        assert_eq!(
            decode_speed(&response([3, 0x41, 0x0d, 88, 0, 0, 0, 0])),
            Ok(88)
        );
        assert_eq!(
            decode_coolant(&response([3, 0x41, 5, 130, 0, 0, 0, 0])),
            Ok(90)
        );
        assert_eq!(
            decode_supported_pids(&response([6, 0x41, 0, 0x80, 0, 0, 1, 0])),
            Ok(0x8000_0001)
        );
    }

    #[test]
    fn mode01_rejects_wrong_metadata_and_negative_responses() {
        let mut frame = response([4, 0x41, 0x0c, 0x2e, 0xe0, 0, 0, 0]);
        frame.id = 0x7e9;
        assert_eq!(decode_rpm(&frame), Err(Error::WrongId));
        assert_eq!(
            decode_rpm(&response([4, 0x42, 0x0c, 0, 0, 0, 0, 0])),
            Err(Error::UnsupportedService)
        );
        assert_eq!(
            decode_rpm(&response([4, 0x41, 0x0d, 0, 0, 0, 0, 0])),
            Err(Error::UnsupportedPid)
        );
        assert_eq!(
            decode_rpm(&response([3, 0x41, 0x0c, 0, 0, 0, 0, 0])),
            Err(Error::ShortPayload)
        );
        assert_eq!(
            decode_rpm(&response([3, 0x7f, 1, 0x12, 0, 0, 0, 0])),
            Err(Error::NegativeResponse {
                service: 1,
                nrc: 0x12
            })
        );
        assert_eq!(
            decode_rpm(&CanFrame {
                id: RESPONSE_ID,
                len: 9,
                data: [0; 8]
            }),
            Err(Error::InvalidLength)
        );
        assert_eq!(
            decode_rpm(&response([0x14, 0x41, 0x0c, 0, 0, 0, 0, 0])),
            Err(Error::Malformed)
        );
        assert_eq!(
            decode_supported_pids(&response([7, 0x41, 0, 0x80, 0, 0, 1, 0])),
            Err(Error::InvalidLength)
        );
        assert_eq!(
            decode_rpm(&response([5, 0x41, 0x0c, 0x2e, 0xe0, 0, 0, 0])),
            Err(Error::InvalidLength)
        );
        assert_eq!(
            decode_speed(&response([4, 0x41, 0x0d, 88, 0, 0, 0, 0])),
            Err(Error::InvalidLength)
        );
        assert_eq!(
            decode_coolant(&response([4, 0x41, 5, 130, 0, 0, 0, 0])),
            Err(Error::InvalidLength)
        );
    }

    #[test]
    fn mode03_dtcs_decode_sae_mapping_and_padding() {
        let result = decode_dtcs(&response([7, 0x43, 0x01, 0x33, 0xc1, 0x23, 0, 0])).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.dtcs[0].ascii(), *b"P0133");
        assert_eq!(result.dtcs[1].ascii(), *b"U0123");
        assert_eq!(
            decode_clear_dtcs(&response([1, 0x44, 0, 0, 0, 0, 0, 0])),
            Ok(())
        );
        assert_eq!(
            decode_dtcs(&response([3, 0x7f, 3, 0x11, 0, 0, 0, 0])),
            Err(Error::NegativeResponse {
                service: 3,
                nrc: 0x11
            })
        );
        assert_eq!(
            decode_clear_dtcs(&response([1, 0x43, 0, 0, 0, 0, 0, 0])),
            Err(Error::UnsupportedService)
        );
    }

    #[test]
    fn vin_reassembly_returns_flow_control_and_exact_vin() {
        let mut rx = VinReassembler::new();
        let ff = response([0x10, 20, 0x49, 2, 1, b'1', b'H', b'G']);
        let flow = rx.push(&ff).unwrap();
        assert_eq!(
            flow,
            IsoTpEvent::FlowControl(CanFrame {
                id: FLOW_CONTROL_ID,
                len: 8,
                data: [0x30, 0, 0, 0, 0, 0, 0, 0]
            })
        );
        assert_eq!(
            rx.push(&response([0x21, b'B', b'H', b'4', b'1', b'J', b'X', b'M'])),
            Ok(IsoTpEvent::Pending)
        );
        assert_eq!(
            rx.push(&response([0x22, b'N', b'1', b'0', b'9', b'1', b'8', b'6'])),
            Ok(IsoTpEvent::Complete(*b"1HGBH41JXMN109186"))
        );
    }

    #[test]
    fn vin_reassembly_rejects_sequence_oversize_and_timeout_without_stale_data() {
        let mut rx = VinReassembler::new();
        assert_eq!(
            rx.push(&response([0x10, 21, 0x49, 2, 1, b'X', b'X', b'X'])),
            Err(Error::Oversize)
        );
        assert_eq!(
            rx.push(&response([0x10, 19, 0x49, 2, 1, b'X', b'X', b'X'])),
            Err(Error::Malformed)
        );
        assert_eq!(
            rx.push(&response([0x10, 20, 0x49, 2, 1, b'1', b'H', b'G'])),
            Ok(IsoTpEvent::FlowControl(CanFrame {
                id: FLOW_CONTROL_ID,
                len: 8,
                data: [0x30, 0, 0, 0, 0, 0, 0, 0]
            }))
        );
        assert_eq!(
            rx.push(&response([0x22, 0, 0, 0, 0, 0, 0, 0])),
            Err(Error::Sequence)
        );
        assert_eq!(
            rx.push(&response([0x21, 0, 0, 0, 0, 0, 0, 0])),
            Err(Error::UnexpectedFrame)
        );
        assert_eq!(
            rx.push(&response([0x10, 20, 0x49, 2, 1, b'1', b'H', b'G'])),
            Ok(IsoTpEvent::FlowControl(CanFrame {
                id: FLOW_CONTROL_ID,
                len: 8,
                data: [0x30, 0, 0, 0, 0, 0, 0, 0]
            }))
        );
        assert_eq!(rx.timeout(), Err(Error::Incomplete));
        rx.reset();
        assert_eq!(rx.timeout(), Ok(()));
    }

    #[test]
    fn vin_reassembly_validates_first_frame_header_before_flow_control() {
        let mut rx = VinReassembler::new();
        assert_eq!(
            rx.push(&response([0x10, 20, 0x48, 2, 1, b'1', b'H', b'G'])),
            Err(Error::UnsupportedService)
        );
        assert_eq!(
            rx.push(&response([0x10, 20, 0x49, 3, 1, b'1', b'H', b'G'])),
            Err(Error::UnsupportedPid)
        );
        assert_eq!(
            rx.push(&response([0x10, 20, 0x49, 2, 2, b'1', b'H', b'G'])),
            Err(Error::Malformed)
        );

        rx.push(&response([0x10, 20, 0x49, 2, 1, b'1', b'H', b'G']))
            .unwrap();
        rx.push(&response([0x21, b'B', b'H', b'4', b'1', b'J', b'X', b'M']))
            .unwrap();
        assert_eq!(
            rx.push(&response([0x22, b'N', b'1', b'0', b'9', b'1', b'8', b'6',])),
            Ok(IsoTpEvent::Complete(*b"1HGBH41JXMN109186"))
        );
    }

    #[test]
    fn vin_reassembly_handles_mode09_negative_single_frames() {
        let mut rx = VinReassembler::new();
        let negative = CanFrame {
            id: RESPONSE_ID,
            len: 4,
            data: [3, 0x7f, 9, 0x11, 0, 0, 0, 0],
        };
        assert_eq!(
            rx.push(&negative),
            Err(Error::NegativeResponse {
                service: 9,
                nrc: 0x11
            })
        );
        let wrong_service = CanFrame {
            id: RESPONSE_ID,
            len: 4,
            data: [3, 0x7f, 1, 0x11, 0, 0, 0, 0],
        };
        assert_eq!(rx.push(&wrong_service), Err(Error::UnsupportedService));
        assert_eq!(
            rx.push(&response([2, 0x7f, 9, 0x11, 0, 0, 0, 0])),
            Err(Error::InvalidLength)
        );
        assert_eq!(
            rx.push(&response([4, 0x7f, 9, 0x11, 0, 0, 0, 0])),
            Err(Error::InvalidLength)
        );
        for dlc in [1, 2, 3, 5] {
            let truncated_or_padded = CanFrame {
                id: RESPONSE_ID,
                len: dlc,
                data: [3, 0x7f, 9, 0x11, 0, 0, 0, 0],
            };
            assert_eq!(rx.push(&truncated_or_padded), Err(Error::InvalidLength));
        }
    }

    #[test]
    fn vin_reassembly_rejects_duplicate_unexpected_and_bad_length_frames() {
        let mut rx = VinReassembler::new();
        let positive_single_frame = CanFrame {
            id: RESPONSE_ID,
            len: 4,
            data: [3, 0x49, 2, 1, 0, 0, 0, 0],
        };
        assert_eq!(rx.push(&positive_single_frame), Err(Error::UnexpectedFrame));
        assert_eq!(
            rx.push(&response([0x30, 0, 0, 0, 0, 0, 0, 0])),
            Err(Error::UnexpectedFrame)
        );

        rx.push(&response([0x10, 20, 0x49, 2, 1, b'1', b'H', b'G']))
            .unwrap();
        rx.push(&response([0x21, b'B', b'H', b'4', b'1', b'J', b'X', b'M']))
            .unwrap();
        assert_eq!(
            rx.push(&response([0x21, 0, 0, 0, 0, 0, 0, 0])),
            Err(Error::Sequence)
        );

        rx.push(&response([0x10, 20, 0x49, 2, 1, b'1', b'H', b'G']))
            .unwrap();
        let mut short_cf = response([0x21, b'B', b'H', b'4', b'1', b'J', b'X', b'M']);
        short_cf.len = 7;
        assert_eq!(rx.push(&short_cf), Err(Error::InvalidLength));

        rx.push(&response([0x10, 20, 0x49, 2, 1, b'1', b'H', b'G']))
            .unwrap();
        let mut overlong_cf = response([0x21, b'B', b'H', b'4', b'1', b'J', b'X', b'M']);
        overlong_cf.len = 9;
        assert_eq!(rx.push(&overlong_cf), Err(Error::InvalidLength));

        rx.push(&response([0x10, 20, 0x49, 2, 1, b'1', b'H', b'G']))
            .unwrap();
        rx.push(&response([0x21, b'B', b'H', b'4', b'1', b'J', b'X', b'M']))
            .unwrap();
        let mut short_final = response([0x22, b'N', b'1', b'0', b'9', b'1', b'8', b'6']);
        short_final.len = 7;
        assert_eq!(rx.push(&short_final), Err(Error::InvalidLength));

        rx.push(&response([0x10, 20, 0x49, 2, 1, b'1', b'H', b'G']))
            .unwrap();
        rx.push(&response([0x21, b'B', b'H', b'4', b'1', b'J', b'X', b'M']))
            .unwrap();
        let mut overlong_final = response([0x22, b'N', b'1', b'0', b'9', b'1', b'8', b'6']);
        overlong_final.len = 9;
        assert_eq!(rx.push(&overlong_final), Err(Error::InvalidLength));
    }

    #[test]
    fn scanner_state_transitions_are_consistent() {
        let mut state = ScannerState::new();
        state.rpm = 3000;
        state.mark_timeout();
        assert_eq!(state.rpm, 3000);
        assert!(state.has(flags::TIMEOUT | flags::STALE));
        state.mark_fresh();
        assert!(state.has(flags::CONNECTED));
        assert!(!state.has(flags::TIMEOUT | flags::STALE));
        assert_eq!(state.generation, 1);
        state.update_dtc_count(2);
        assert!(state.has(flags::DTC_PRESENT));
        state.update_dtc_count(0);
        assert!(!state.has(flags::DTC_PRESENT));
        state.set_vin(*b"1HGBH41JXMN109186");
        assert!(state.vin_valid);
    }
}
