// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.

//! Liveness gate for the silent-path census.
//!
//! A census that reports zero is only meaningful if the counter is capable of
//! reporting non-zero. This drives a *real* peripheral model at an offset the
//! model demonstrably does not decode and asserts the counter moves — so a
//! "clean" row in `docs/coverage/silent-path-census.md` means "the firmware did
//! not take this path", never "the instrumentation was dead".
//!
//! The whole file compiles away without the `silent-census` feature, because
//! `census::reset`/`to_json` only exist when it is on. Run it with:
//! `cargo test -p labwired-core --features silent-census --test census_probe`

#![cfg(feature = "silent-census")]

use labwired_core::Peripheral;

/// The F1 RCC model decodes 0x00..=0x28 and nothing else (RM0008 §7.3), so
/// 0xF0 is a known-undecoded offset. Reading it must fabricate a zero *and*
/// leave a census entry behind.
#[test]
fn undecoded_rcc_offset_is_counted_and_still_reads_zero() {
    labwired_core::census::reset();
    let mut rcc = labwired_core::peripherals::rcc::Rcc::new();

    // A decoded offset must NOT be counted: this is the negative control that
    // stops the counter from being trivially always-on.
    assert_eq!(rcc.read_u32(0x00).unwrap(), 0x0000_4A83, "CR reset value");
    assert_eq!(
        labwired_core::census::to_json()["undecoded_register_access"]["total"],
        0,
        "a decoded offset must never be recorded as undecoded"
    );

    // The undecoded offset still behaves exactly as before instrumentation:
    // the read fabricates zero and the write is discarded.
    assert_eq!(rcc.read_u32(0xF0).unwrap(), 0);
    rcc.write_u32(0xF0, 0xDEAD_BEEF).unwrap();
    assert_eq!(rcc.read_u32(0xF0).unwrap(), 0, "write must stay discarded");

    let j = labwired_core::census::to_json();
    let total = j["undecoded_register_access"]["total"].as_u64().unwrap();
    assert!(total > 0, "census_reg! never fired on an undecoded offset");

    let entries = j["undecoded_register_access"]["entries"]
        .as_array()
        .unwrap()
        .clone();
    assert!(
        entries
            .iter()
            .any(|e| e["peripheral"] == "rcc:F1Rcc" && e["offset"] == "0x00f0"),
        "expected an rcc:F1Rcc @ 0x00f0 entry, got {entries:?}"
    );
}

/// Documents the byte-granularity multiplier that the raw counts carry, so a
/// reader of the census table divides by the right number.
///
/// `Peripheral::read`/`write` are byte-granular and `read_u32`/`write_u32`
/// decompose into four byte accesses. On top of that, `Rcc::write` is a
/// read-modify-write: each of the four byte writes first calls `read_reg`.
/// So ONE 32-bit undecoded register write costs 4 write-hits AND 4 read-hits,
/// and one 32-bit undecoded read costs 4 read-hits.
#[test]
fn raw_counts_carry_a_four_times_byte_multiplier() {
    labwired_core::census::reset();
    let mut rcc = labwired_core::peripherals::rcc::Rcc::new();

    rcc.write_u32(0xF0, 0x1234_5678).unwrap();
    let j = labwired_core::census::to_json();
    let entries = j["undecoded_register_access"]["entries"]
        .as_array()
        .unwrap();
    let get = |kind: &str| -> u64 {
        entries
            .iter()
            .find(|e| e["kind"] == kind)
            .and_then(|e| e["count"].as_u64())
            .unwrap_or(0)
    };
    assert_eq!(get("write"), 4, "one u32 write == four byte writes");
    assert_eq!(get("read"), 4, "…each preceded by a read-modify-write read");
}
