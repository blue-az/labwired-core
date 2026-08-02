// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! END-TO-END GATE FOR THE ESP32-C3 BLE BASEBAND.
//!
//! What it proves
//! ==============
//! A REAL Arduino-ESP32 BLE sketch, compiled by the hosted builder against
//! framework-arduinoespressif32 / esp-idf v4.4.7, boots through the genuine C3
//! mask ROM from a flash image and gets `BLEDevice::init()` AND
//! `startAdvertising()` to RETURN inside the twin. The sketch narrates itself:
//!
//!     PRE_BLE       before BLEDevice::init()
//!     BLE_INIT_OK   init() returned
//!     ADV_ON        startAdvertising() returned
//!     ALIVE         loop() is running
//!
//! Those strings are the firmware's own `Serial.println`s coming out of the
//! modelled UART0 — nothing in the harness can produce them. The gate asserts
//! the middle two.
//!
//! Why it exists
//! =============
//! `crates/core/src/peripherals/esp32c3/bt.rs` models the BLE baseband window
//! at 0x6003_1000. Before it existed, this firmware died with a memory
//! violation at 0x6003_1204 partway through `PRE_BLE`; before `RWBLECNTL`
//! bit31 was made self-clearing it wedged forever on the controller's own
//! kick. Both failures are invisible to every unit test in the tree — they
//! only show up when the compiled Bluedroid stack drives the model for a few
//! hundred million steps. Without this file the model could regress to either
//! state and nothing would say so.
//!
//! It is deliberately an END-TO-END gate, not a register test. The unit tests
//! in `bt.rs` assert what the model does with a write; this asserts that the
//! firmware gets through, which is the only claim anyone cares about.
//!
//! The firmware is NOT committed
//! =============================
//! The composed flash image is 4 MiB. It is fetched from the content-addressed
//! blob store and digest-pinned by `scripts/ci/fetch-c3-ble-flash.sh` +
//! `scripts/ci/c3-ble-flash.sha256`. This test re-verifies the digest itself
//! before booting, so a hand-placed or half-written file cannot masquerade as
//! the pinned firmware and turn this gate into a measurement of something
//! nobody reviewed.
//!
//! Skip vs require
//! ===============
//! Absent firmware SKIPS, so a standalone checkout with no network stays green
//! — but `LABWIRED_REQUIRE_C3_BLE=1` turns absence into a hard failure. CI sets
//! it. A gate that can silently skip in CI is not a gate; that is the exact
//! hole this file was written to close. (Same contract as
//! `LABWIRED_REQUIRE_EREADER_ELF` on the e-reader gate.)
//!
//! Running it
//! ==========
//! `#[ignore]`d because it is a ~360M-step faithful ROM boot. Debug is far too
//! slow to be useful; run it in release:
//!
//!     scripts/ci/fetch-c3-ble-flash.sh
//!     cargo test --release -p labwired-cli --test e2e_esp32c3_ble_arduino \
//!         -- --ignored --nocapture

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Repo root = crates/cli/../.. (matches the other CLI integration tests).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonicalize repo root")
}

/// Where `scripts/ci/fetch-c3-ble-flash.sh` writes the composed image by
/// default. `.gitignore` covers `fixtures/`, so nothing here can be committed
/// by accident.
const DEFAULT_FLASH: &str = "fixtures/esp32c3-ble/esp32c3-ble-arduino-flash.bin";

/// Step budget.
///
/// MEASURED on this tree against the pinned image: `ADV_ON` leaves the UART TX
/// shift register at 362_274_218 steps / 960_531_682 cycles. Most of that is
/// not the BLE stack — it is the genuine mask ROM, the 2nd-stage bootloader,
/// ESP-IDF `cpu_start`, and the console shifting bytes out of UART0 at the
/// modelled 115200-baud rate (see `no_elf_c3_rom_boot.rs` for that arithmetic).
///
/// 420M is that number plus ~16 % headroom. It is a CEILING, not a target: the
/// script sets `stop_when_assertions_pass`, so a healthy run halts the moment
/// both markers have been seen (plus the 100k-step settle window that catches
/// print-then-crash) rather than grinding the budget. The ceiling only bounds
/// how long a BROKEN model is allowed to flail before the gate calls it.
const BLE_ADV_MAX_STEPS: u64 = 420_000_000;

/// Parse the `image` row of the committed digest manifest.
///
/// ONE source of truth: the fetch script and this test both read the same file,
/// so the pin cannot drift between "what CI downloads" and "what the gate
/// agrees to boot".
fn pinned_image_sha(root: &Path) -> String {
    let manifest = root.join("scripts/ci/c3-ble-flash.sha256");
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

/// Resolve the BLE probe flash image, or `None` to skip.
///
/// `LABWIRED_C3_BLE_FLASH` overrides the path; otherwise the default fetch
/// destination is used. Returns `None` only when the file is absent AND
/// `LABWIRED_REQUIRE_C3_BLE` is not `1`.
fn resolve_flash(root: &Path) -> Option<PathBuf> {
    let path = std::env::var("LABWIRED_C3_BLE_FLASH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join(DEFAULT_FLASH));
    if path.exists() {
        return Some(path);
    }
    if std::env::var("LABWIRED_REQUIRE_C3_BLE").as_deref() == Ok("1") {
        panic!(
            "ESP32-C3 BLE probe flash not found at {} but LABWIRED_REQUIRE_C3_BLE=1 \
             — the BLE baseband model is UNGUARDED. Run \
             `scripts/ci/fetch-c3-ble-flash.sh` first, or point \
             LABWIRED_C3_BLE_FLASH at the pinned image.",
            path.display()
        );
    }
    eprintln!(
        "[skip] ESP32-C3 BLE probe flash not found at {}; run \
         `scripts/ci/fetch-c3-ble-flash.sh` (needs network) to enable this gate",
        path.display()
    );
    None
}

/// Write the `labwired test` script.
///
/// `firmware: ""` is deliberate — the schema requires the key, the CLI filters
/// it empty and takes the ELF-less rom-boot path, which is how the hosted
/// builder runs rom-boot chips (a multi-MB debug ELF overflows the D1 blob
/// row). The flash image IS the program.
///
/// `stop_when_assertions_pass` is the acceptance stop: the run ends when both
/// markers have been seen, not when the budget is exhausted.
/// `expected_stop_reason: assertions_passed` is asserted so that "ran to
/// max_steps having printed the markers by luck of a later run" cannot pass —
/// and so the early stop itself is under test.
fn write_script(dir: &Path, system: &Path) -> PathBuf {
    let script = format!(
        "schema_version: \"1.0\"\n\
         inputs:\n  \
           firmware: \"\"\n  \
           system: \"{}\"\n\
         limits:\n  \
           max_steps: {BLE_ADV_MAX_STEPS}\n  \
           stop_when_assertions_pass: true\n\
         assertions:\n  \
           - uart_contains: \"BLE_INIT_OK\"\n  \
           - uart_contains: \"ADV_ON\"\n  \
           - expected_stop_reason: assertions_passed\n",
        system.display(),
    );
    let path = dir.join("c3_ble_romboot.yaml");
    std::fs::write(&path, script).expect("write test script");
    path
}

#[test]
#[ignore = "faithful C3 ROM boot of a real Arduino BLE binary (~360M steps); run with --release --ignored"]
fn esp32c3_ble_arduino_reaches_ble_init_and_advertising() {
    let root = repo_root();
    let Some(flash) = resolve_flash(&root) else {
        return;
    };

    // Verify the pin BEFORE booting. An unpinned image would make every
    // assertion below a statement about an unknown binary.
    let want = pinned_image_sha(&root);
    let got = sha256_file(&flash);
    assert_eq!(
        got,
        want,
        "{} is NOT the pinned BLE probe image.\n  expected {want}\n  got      {got}\n\
         Re-run scripts/ci/fetch-c3-ble-flash.sh. If the probe firmware was \
         rebuilt on purpose, update scripts/ci/c3-ble-flash.sha256 in the same \
         change and say why.",
        flash.display(),
    );
    eprintln!("[c3-ble] flash {} (sha256 {got})", flash.display());

    // Plain devkit: no external devices. The BLE baseband is on-chip, declared
    // by configs/chips/esp32c3.yaml, so nothing has to be attached for this.
    let system = root.join("configs/systems/esp32c3-devkit.yaml");
    assert!(system.exists(), "missing system: {}", system.display());

    let tmp = std::env::temp_dir().join(format!("lw-c3-ble-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create tmp dir");
    let script = write_script(&tmp, &system);
    let out_dir = tmp.join("out");
    std::fs::create_dir_all(&out_dir).expect("create out dir");

    let started = std::time::Instant::now();
    // Exactly as the builder invokes rom-boot chips: `--rom-boot`, the flash
    // image via the env pin, NO `--firmware`.
    let output = Command::new(env!("CARGO_BIN_EXE_labwired"))
        .current_dir(&root)
        .env("LABWIRED_ESP32C3_FLASH", &flash)
        .args([
            "test",
            "--script",
            script.to_str().unwrap(),
            "--rom-boot",
            "--no-uart-stdout",
            "--no-key",
            "--output-dir",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("spawn labwired");
    let elapsed = started.elapsed();

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let uart = std::fs::read_to_string(out_dir.join("uart.log")).unwrap_or_default();
    let result_json = std::fs::read_to_string(out_dir.join("result.json")).unwrap_or_default();

    eprintln!("[c3-ble] wall time: {:.1}s", elapsed.as_secs_f64());
    eprintln!("[c3-ble] ── firmware serial ────────────────────────────────");
    for line in uart.lines() {
        eprintln!("[c3-ble] | {line}");
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&result_json) {
        eprintln!(
            "[c3-ble] status={} steps={} cycles={} stop_reason={}",
            v["status"], v["steps_executed"], v["cycles"], v["stop_reason"],
        );
    }

    // The ELF-less branch must have been taken (not a silent firmware
    // fallback) — otherwise this would be measuring some other boot path.
    assert!(
        stderr.contains("ELF-less"),
        "expected the ELF-less C3 rom-boot branch to run; stderr:\n{stderr}"
    );
    assert!(
        !result_json.is_empty(),
        "no result.json (exit {:?}) — the sim never ran.\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(
        !result_json.contains("\"config_error\""),
        "run ended in a config_error:\n{result_json}\nstderr:\n{stderr}"
    );

    // The verdict. The firmware's own markers, from the modelled UART. Assert
    // them here as well as in the script so the failure message names WHICH
    // stage of BLE bring-up died, instead of "an assertion failed".
    assert!(
        uart.contains("PRE_BLE"),
        "firmware never reached the pre-BLE marker — this is a boot failure, \
         not a BLE failure. UART ({} bytes):\n{uart}\nstderr:\n{stderr}",
        uart.len(),
    );
    assert!(
        uart.contains("BLE_INIT_OK"),
        "BLEDevice::init() never returned: the twin printed PRE_BLE and then \
         stopped inside BLE bring-up. Check the 0x6003_1000 baseband model \
         (crates/core/src/peripherals/esp32c3/bt.rs) — a memory violation in \
         that window means a register went missing, and a run that ends at \
         max_steps with the CPU parked on one PC means a status/command bit \
         stopped answering. UART ({} bytes):\n{uart}\nstderr:\n{stderr}",
        uart.len(),
    );
    assert!(
        uart.contains("ADV_ON"),
        "BLEDevice::init() completed but startAdvertising() never returned. \
         UART ({} bytes):\n{uart}\nstderr:\n{stderr}",
        uart.len(),
    );

    assert!(
        output.status.success(),
        "the BLE rom-boot run failed (exit {:?}).\nresult.json:\n{result_json}\n\
         stderr:\n{stderr}",
        output.status.code(),
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
