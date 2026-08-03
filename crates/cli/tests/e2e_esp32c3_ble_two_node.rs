// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! END-TO-END GATE FOR THE ESP32-C3 BLE **RECEIVE** PATH.
//!
//! What it proves
//! ==============
//! Two ESP32-C3 twins, each booting the genuine mask ROM and the same real
//! Arduino-ESP32 binary from flash, exchange application data over the shared
//! BLE air — in BOTH directions, connectionless, through the whole stack:
//!
//!     app -> BLEAdvertising -> Bluedroid -> HCI -> RW-BLE link layer ->
//!     exchange memory -> the modelled baseband -> the air ->
//!     the peer's baseband -> its RX descriptor ring -> lld_scan ->
//!     Bluedroid -> BLEAdvertisedDeviceCallbacks::onResult -> app
//!
//! The sketch derives its own tag from `BLEDevice::getAddress()` and puts
//! `E5 02 <tag> <counter>` in its manufacturer data, so:
//!
//!     [A] MYTAG 4          A's own BLE address ends 04
//!     [B] MYTAG 5          B's ends 05
//!     [A] PEER tag=5 ...   A's application read B's advertisement
//!     [B] PEER tag=4 ...   B's application read A's advertisement
//!
//! Those `PEER` lines are the firmware's own `Serial.print`s out of the
//! modelled UART0, from inside an `onResult` callback that only fires when
//! Bluedroid delivers a real advertising report. Nothing in the harness can
//! produce them.
//!
//! Why it exists
//! =============
//! `crates/core/src/peripherals/esp32c3/bt.rs` had a working *transmit* path
//! and a controller that demonstrably received — and still delivered zero
//! advertising reports to any host, for four passes. The reason was three bits
//! of RX-descriptor ownership protocol (`+0x00` bit15 `RXDONE`, `+0x02` bit15
//! "released", `+0x0C` bits[15:11] link label) that `r_lld_rxdesc_check` gates
//! the report on. Every one of them is invisible to a register test and to a
//! trace of the model: the trace showed healthy receptions the whole time. Only
//! a real stack running end to end says whether the host heard anything, which
//! is exactly what this file asserts.
//!
//! The firmware is NOT committed
//! =============================
//! Digest-pinned in `scripts/ci/c3-ble-node-flash.sha256` and fetched by
//! `scripts/ci/fetch-c3-ble-flash.sh <dest> <manifest>`. This test re-verifies
//! the composed image's digest before booting it, so an unreviewed binary
//! cannot turn the gate into a measurement of something else.
//!
//! Skip vs require
//! ===============
//! Absent firmware SKIPS so an offline checkout stays green;
//! `LABWIRED_REQUIRE_C3_BLE=1` turns absence into a hard failure, the same
//! contract `e2e_esp32c3_ble_arduino.rs` holds.
//!
//! Running it
//! ==========
//! `#[ignore]`d: two faithful ROM boots stepping in lockstep is a long run.
//!
//!     scripts/ci/fetch-c3-ble-flash.sh fixtures/esp32c3-ble \
//!         scripts/ci/c3-ble-node-flash.sha256
//!     cargo test --release -p labwired-cli --test e2e_esp32c3_ble_two_node \
//!         -- --ignored --nocapture

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonicalize repo root")
}

/// Where the fetch script writes the composed two-way node image.
const DEFAULT_FLASH: &str = "fixtures/esp32c3-ble/esp32c3-ble-node-flash.bin";

/// Step budget — a CEILING, not a target.
///
/// The run stops the moment BOTH nodes have printed a `PEER` line
/// (`LABWIRED_BLE_DUAL_STOP_ON`), so a healthy engine never reaches this. It
/// only bounds how long a broken one is allowed to flail. MEASURED on this
/// tree: both nodes had reported by **44 M steps**. The ceiling is more than an
/// order of magnitude above that, which is deliberate — the variance here is
/// the advertising interval's random delay deciding when the two nodes'
/// channel-39 dwells overlap, and a gate that fails because two pseudo-random
/// schedules took a few extra intervals to line up is a flake, not a finding.
const TWO_NODE_MAX_STEPS: u64 = 600_000_000;

/// Substring both nodes must print before the run may stop early.
const STOP_ON: &str = "PEER tag=";

fn pinned_image_sha(root: &Path) -> String {
    let manifest = root.join("scripts/ci/c3-ble-node-flash.sha256");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut cols = line.split_whitespace();
        let (Some(sha), Some(off)) = (cols.next(), cols.next()) else {
            continue;
        };
        if off == "image" {
            return sha.to_string();
        }
    }
    panic!("{} has no `image` row", manifest.display());
}

fn sha256_file(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut h = Sha256::new();
    h.update(&bytes);
    format!("{:x}", h.finalize())
}

fn resolve_flash(root: &Path) -> Option<PathBuf> {
    let path = std::env::var("LABWIRED_C3_BLE_NODE_FLASH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join(DEFAULT_FLASH));
    if path.exists() {
        return Some(path);
    }
    if std::env::var("LABWIRED_REQUIRE_C3_BLE").as_deref() == Ok("1") {
        panic!(
            "ESP32-C3 BLE two-way node flash not found at {} but \
             LABWIRED_REQUIRE_C3_BLE=1 — the BLE RECEIVE path is UNGUARDED. Run \
             `scripts/ci/fetch-c3-ble-flash.sh fixtures/esp32c3-ble \
             scripts/ci/c3-ble-node-flash.sha256` first.",
            path.display()
        );
    }
    eprintln!(
        "[skip] ESP32-C3 BLE node flash not found at {}; run \
         `scripts/ci/fetch-c3-ble-flash.sh fixtures/esp32c3-ble \
         scripts/ci/c3-ble-node-flash.sha256` (needs network) to enable this gate",
        path.display()
    );
    None
}

#[test]
#[ignore = "two faithful C3 ROM boots stepping in lockstep over the shared BLE air; run with --release --ignored"]
fn two_c3_nodes_exchange_advertising_data_at_the_application_level() {
    let root = repo_root();
    let Some(flash) = resolve_flash(&root) else {
        return;
    };

    let want = pinned_image_sha(&root);
    let got = sha256_file(&flash);
    assert_eq!(
        got,
        want,
        "{} is NOT the pinned BLE node image.\n  expected {want}\n  got      {got}\n\
         Re-run scripts/ci/fetch-c3-ble-flash.sh with \
         scripts/ci/c3-ble-node-flash.sha256. If the sketch was rebuilt on \
         purpose, update that manifest in the same change and say why.",
        flash.display(),
    );
    eprintln!("[c3-ble-2n] flash {} (sha256 {got})", flash.display());

    let started = std::time::Instant::now();
    // Both nodes boot the SAME binary; the twin gives them different factory
    // MACs, which is what makes their BLE addresses — and therefore the tags
    // they advertise — differ. `--firmware` is only there to satisfy the arg
    // parser: `--rom-boot` takes the ELF-less path and the flash image IS the
    // program.
    let output = Command::new(env!("CARGO_BIN_EXE_labwired"))
        .current_dir(&root)
        .env("LABWIRED_BLE_DUAL", "1")
        .env("LABWIRED_ESP32C3_FLASH", &flash)
        .env("LABWIRED_ESP32C3_FLASH_B", &flash)
        .env("LABWIRED_BLE_DUAL_STOP_ON", STOP_ON)
        .args([
            "run",
            "--rom-boot",
            "--chip",
            "configs/chips/esp32c3.yaml",
            "--firmware",
            flash.to_str().unwrap(),
            "--max-steps",
            &TWO_NODE_MAX_STEPS.to_string(),
        ])
        .output()
        .expect("spawn labwired");
    let elapsed = started.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    eprintln!("[c3-ble-2n] wall time: {:.1}s", elapsed.as_secs_f64());
    eprintln!("[c3-ble-2n] ── firmware serial (both nodes) ──────────────────");
    for line in stdout.lines() {
        eprintln!("[c3-ble-2n] | {line}");
    }

    // Both stacks must come up at all — otherwise a "no report" failure below
    // would be a bring-up regression wearing a receive-path costume.
    for marker in ["BLE_INIT_OK", "ADV_ON", "SCAN_ON"] {
        for node in ["[A]", "[B]"] {
            assert!(
                stdout
                    .lines()
                    .any(|l| l.starts_with(node) && l.contains(marker)),
                "node {node} never printed {marker} — this is a BLE bring-up \
                 failure, not a receive-path failure.\nserial:\n{stdout}\nstderr:\n{stderr}"
            );
        }
    }

    // Each node's tag is the last byte of its own BLE address, and the twin
    // assigns node A factory MAC ...:02 and node B ...:03 (BLE address = base
    // MAC + 2), so A is tag 4 and B is tag 5. Assert the identities before
    // asserting who heard whom, so a MAC-assignment change fails HERE with a
    // clear message instead of as a mysterious missing report.
    assert!(
        stdout.contains("[A] MYTAG 4"),
        "node A tag\nserial:\n{stdout}"
    );
    assert!(
        stdout.contains("[B] MYTAG 5"),
        "node B tag\nserial:\n{stdout}"
    );

    // The verdict: each application read the OTHER node's advertisement.
    // Cross-checked on the tag AND the source address, so a report echoing a
    // node's own transmission back at it could not pass.
    assert!(
        stdout.lines().any(|l| l.starts_with("[A]")
            && l.contains("PEER tag=5")
            && l.contains("from=02:00:00:00:00:05")),
        "node A's application never saw node B's advertisement. The controller \
         may be receiving fine — check the RX descriptor ownership bits in \
         crates/core/src/peripherals/esp32c3/bt.rs (RXD_DONE, \
         RXD_STATUS_RELEASED, RXD_LINK_LABEL): r_lld_rxdesc_check silently \
         reports nothing when any of them is wrong.\nserial:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("[B]") && l.contains("PEER tag=4") && l.contains("from=02:00:00:00:00:04")),
        "node B's application never saw node A's advertisement.\nserial:\n{stdout}\nstderr:\n{stderr}"
    );

    // And the run must have stopped on the acceptance condition rather than
    // grinding the ceiling — otherwise the budget, not the engine, is what this
    // gate is measuring.
    assert!(
        stderr.contains("both nodes printed"),
        "the run used its whole step ceiling instead of stopping on {STOP_ON:?}\nstderr:\n{stderr}"
    );
}
