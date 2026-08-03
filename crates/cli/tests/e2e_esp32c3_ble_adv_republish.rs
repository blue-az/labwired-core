// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! END-TO-END GATE FOR **CONTINUOUS** BLE ADVERTISING RE-PUBLICATION.
//!
//! What it proves
//! ==============
//! `e2e_esp32c3_ble_two_node.rs` proves two C3 twins exchange application data
//! over modelled BLE, in both directions, with the bytes changing **once**
//! (`val=0` from `setup()`, `val=1` from the first `loop()`). That is enough
//! to call the receive path real and nowhere near enough to run an
//! application: a game whose paddle moves has to re-publish forever.
//!
//! This gate is the forever part. The same pinned node image runs
//! `stop() → setAdvertisementData(counter) → start()` on a 700 ms cadence, and
//! this test requires **five complete `loop()` iterations on each node AND five
//! different successive counter values read by the peer's application** —
//! `val=0` through `val=4`, each one out of a different iteration, both ways.
//!
//! The blocker it was written against
//! ==================================
//! Nothing was ever *blocked*. The loop ran, on time — `TICK n` lands every
//! 112 M steps, which is the sketch's own `delay(700)` to the FreeRTOS tick.
//! What happened instead is that the twin **died with the link layer's own
//! assertion** at ~198 M steps, i.e. two iterations in:
//!
//! ```text
//! [A] assert ble_util_buf.c 180, param 000000e2 00000205
//! [B] assert ble_util_buf.c 180, param 000000e2 00000204
//! ```
//!
//! The model masked RX descriptor `+0x12` — the received-payload buffer
//! pointer — with `0x7FFF`, on a bit15 convention that belongs to `+0x0`
//! (`RXDONE`) and to nothing else. The RX buffer pool's last five buffers are
//! at `0x8005 .. 0x9805`, so the mask folded them onto exchange memory's first
//! 8 KiB and the model wrote received advertising payloads over the exchange
//! table, the control structures and the descriptor ring. `0x0205` is what the
//! ring's own descriptor 0 ended up holding: the peer's `<tag> <counter>`
//! bytes, which is why node A's number and node B's differ by exactly the tag.
//! Full ROM chain at `RXD_DATA_PTR` in
//! `crates/core/src/peripherals/esp32c3/bt.rs`.
//!
//! Before that was understood the same run died one assert earlier
//! (`assert emi.c 159, param 0000ff33 0000003f`) on a stale RX-descriptor
//! `+0xE`, which the aliased writes had produced. Both are covered here for the
//! same reason: the check below is on the word `assert`, not on a message.
//!
//! That is why this file asserts on `assert ` appearing in the serial at all:
//! the firmware's own assertion is the sharpest oracle in this whole stack, it
//! is what caught both defects this gate was built around, and it fires
//! HUNDREDS of millions of steps in — long after every acceptance marker a
//! laxer gate would stop on. A run that prints five values and then asserts has
//! not passed.
//!
//! The firmware is NOT committed
//! =============================
//! Same pinned image as `e2e_esp32c3_ble_two_node.rs`
//! (`scripts/ci/c3-ble-node-flash.sha256`, fetched by
//! `scripts/ci/fetch-c3-ble-flash.sh <dest> <manifest>`), re-verified here
//! before booting.
//!
//! ⚠ IT NEEDS `--features event-scheduler`, AND IT IS COMPILED OUT WITHOUT IT
//! ========================================================================
//! Identical contract to the two-node gate: the radio engine runs from
//! `Peripheral::on_event`, so without the feature this file is an EMPTY test
//! binary that reports "ok. 0 passed" — green while measuring nothing. The
//! workflow step MUST pass the feature.
//!
//! Running it
//! ==========
//!     scripts/ci/fetch-c3-ble-flash.sh fixtures/esp32c3-ble \
//!         scripts/ci/c3-ble-node-flash.sha256
//!     cargo test --release --features event-scheduler -p labwired-cli \
//!         --test e2e_esp32c3_ble_adv_republish -- --ignored --nocapture
#![cfg(feature = "event-scheduler")]

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

/// How many DIFFERENT successive counter values the peer's application must
/// read (`val=0` .. `val=VALUES_REQUIRED-1`). Five is the acceptance the whole
/// exercise was set against: one value is a report, two is a data exchange,
/// five is a stream an application can be built on.
const VALUES_REQUIRED: u32 = 5;

/// How many `loop()` iterations must complete. `val=k` is what iteration `k`
/// publishes, so requiring `TICK 5` is strictly more than requiring five
/// values — and it is the half of the property the advertiser owns: five
/// complete `stop() → setAdvertisementData() → start()` cycles that all
/// returned.
const TICKS_REQUIRED: u32 = 5;

/// Step budget — a CEILING, not a target.
///
/// MEASURED on this tree: the sketch enters `loop()` at ~42 M steps and its
/// `delay(700)` is 700 ms = **112 M steps** of device time (the twin runs
/// 160 M steps per simulated second — 5 M steps was measured at 31 FreeRTOS
/// ticks, and the scanner's 2.8125 ms event cadence lands on the same number
/// independently). The full observed schedule, both nodes in lockstep:
///
/// ```text
/// TICK 1  42.4 M   TICK 2 154.6 M   TICK 3 266.8 M
/// TICK 4 379.7 M   TICK 5 491.9 M   `val=5` read by the peer 492.9 M
/// ```
///
/// so the acceptance is reached at ~493 M and the run stops there.
///
/// The ceiling is deliberately closer to the target than the two-node gate's,
/// and for a reason: there the variance was the advertising interval's random
/// delay deciding when two nodes' channel-39 dwells first overlap, which is
/// genuinely spread out. Here the cadence is a `vTaskDelay` — deterministic to
/// the tick — and the peer reads each value within ~1 M steps of it being
/// published. A ceiling an order of magnitude above the target would only buy
/// a 25-minute failure instead of a 14-minute one.
const REPUBLISH_MAX_STEPS: u64 = 800_000_000;

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
             LABWIRED_REQUIRE_C3_BLE=1 — CONTINUOUS advertising re-publication \
             is UNGUARDED. Run `scripts/ci/fetch-c3-ble-flash.sh \
             fixtures/esp32c3-ble scripts/ci/c3-ble-node-flash.sha256` first.",
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
#[ignore = "two faithful C3 ROM boots re-publishing advertising data for five loop() iterations; run with --release --ignored"]
fn an_application_republishes_advertising_data_and_the_peer_reads_every_value() {
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
         scripts/ci/c3-ble-node-flash.sha256.",
        flash.display(),
    );
    eprintln!("[c3-ble-rp] flash {} (sha256 {got})", flash.display());

    // Stop as soon as BOTH nodes' applications have read the value published
    // by the LAST iteration this gate requires. It is deliberately one past
    // `VALUES_REQUIRED`: `val=k` is read shortly after `TICK k`, so waiting for
    // `val=5` is what makes reaching the acceptance imply BOTH halves — five
    // completed `loop()` iterations on the advertiser AND five different values
    // already read by the peer — with one poll rather than two.
    //
    // Measured on this tree (deterministic, both nodes in lockstep):
    // TICK 1 @ 42.4 M, TICK 2 @ 154.6 M, TICK 3 @ 266.8 M, TICK 4 @ 379.7 M,
    // TICK 5 @ 491.9 M, `val=5` read @ 492.9 M. The 112 M step spacing is the
    // sketch's own `delay(700)`.
    let stop_on = format!("val={TICKS_REQUIRED}");

    let started = std::time::Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_labwired"))
        .current_dir(&root)
        .env("LABWIRED_BLE_DUAL", "1")
        .env("LABWIRED_ESP32C3_FLASH", &flash)
        .env("LABWIRED_ESP32C3_FLASH_B", &flash)
        .env("LABWIRED_BLE_DUAL_STOP_ON", &stop_on)
        .args([
            "run",
            "--rom-boot",
            "--chip",
            "configs/chips/esp32c3.yaml",
            "--firmware",
            flash.to_str().unwrap(),
            "--max-steps",
            &REPUBLISH_MAX_STEPS.to_string(),
        ])
        .output()
        .expect("spawn labwired");
    let elapsed = started.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    eprintln!("[c3-ble-rp] wall time: {:.1}s", elapsed.as_secs_f64());
    eprintln!("[c3-ble-rp] ── firmware serial (both nodes) ──────────────────");
    for line in stdout.lines() {
        eprintln!("[c3-ble-rp] | {line}");
    }

    // Bring-up first, so a stack that never came up fails HERE rather than as a
    // mysterious missing counter value.
    for marker in ["BLE_INIT_OK", "ADV_ON", "SCAN_ON"] {
        for node in ["[A]", "[B]"] {
            assert!(
                stdout
                    .lines()
                    .any(|l| l.starts_with(node) && l.contains(marker)),
                "node {node} never printed {marker} — BLE bring-up failure, not \
                 a re-publication failure.\nserial:\n{stdout}\nstderr:\n{stderr}"
            );
        }
    }

    // THE FIRMWARE'S OWN ASSERTION IS THE SHARPEST ORACLE IN THIS STACK.
    //
    // The RW-BLE link layer asserts with `assert <file> <line>, param <a> <b>`
    // on the same UART the sketch prints to. Every real defect this model has
    // had that survived its unit tests announced itself that way, and the one
    // ones this gate exists for (`assert ble_util_buf.c 180, param 000000e2
    // 00000205`, a received payload written over the descriptor ring; and
    // `assert emi.c 159, param 0000ff33 0000003f`, the stale descriptor field
    // that produced) fire HUNDREDS of millions of steps into the run — long
    // after the acceptance markers a laxer gate would stop on. Check it before
    // the counters, so the message names the real failure. The check is on the
    // WORD, not on either message.
    for line in stdout.lines() {
        assert!(
            !line.contains("assert "),
            "the firmware asserted — the link layer is being fed something the \
             real core would never produce:\n    {line}\n\
             Check the RX descriptor write-back in \
             crates/core/src/peripherals/esp32c3/bt.rs. Two things it must get \
             right: descriptor payload pointers (+0x12, +0x4, CS+0x1C) are FULL \
             16-bit exchange-memory offsets — masking bit15 aliases the RX pool \
             onto EM 0x0000..0x1FFF — and every CORE-OWNED field (+0x2, +0x4, \
             +0x6, +0x8, +0xC, +0xE, +0x10, +0x0 bit15) must be written on every \
             reception, because a field left alone is stale, not unmodelled, and \
             the link layer cannot tell the \
             difference.\nserial:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    // Identities, so a MAC-assignment change fails with a clear message.
    assert!(
        stdout.contains("[A] MYTAG 4"),
        "node A tag\nserial:\n{stdout}"
    );
    assert!(
        stdout.contains("[B] MYTAG 5"),
        "node B tag\nserial:\n{stdout}"
    );

    // 1. The application's own loop keeps running. `TICK n` is printed at the
    //    END of iteration n, after stop/setAdvertisementData/start have all
    //    returned, so `TICK 5` present means five complete re-publications.
    for node in ["[A]", "[B]"] {
        for n in 1..=TICKS_REQUIRED {
            assert!(
                stdout
                    .lines()
                    .any(|l| l.starts_with(node) && l.trim_end() == format!("{node} TICK {n}")),
                "node {node} never reached loop() iteration {n} — an \
                 application that can publish once and not again cannot carry \
                 live state.\nserial:\n{stdout}\nstderr:\n{stderr}"
            );
        }
    }

    // 2. And the PEER read every one of those values. This is the property a
    //    game needs and the reason the gate is not just "TICK climbs": the
    //    advertiser's own print proves nothing about what crossed the air.
    //    Cross-checked on the tag AND the source address so a node hearing its
    //    own transmission could not pass.
    for (node, peer_tag, peer_addr) in [
        ("[A]", "PEER tag=5", "from=02:00:00:00:00:05"),
        ("[B]", "PEER tag=4", "from=02:00:00:00:00:04"),
    ] {
        for v in 0..VALUES_REQUIRED {
            // Trailing space on purpose: the line is
            // `PEER tag=<t> val=<v> from=<addr>`, so `val=1 ` cannot also match
            // `val=10`.
            let val = format!("val={v} ");
            assert!(
                stdout.lines().any(|l| l.starts_with(node)
                    && l.contains(peer_tag)
                    && l.contains(&val)
                    && l.contains(peer_addr)),
                "node {node}'s application never read {val} from its peer \
                 ({peer_tag} {peer_addr}). It needs {VALUES_REQUIRED} DIFFERENT \
                 successive values, which is what separates a data stream from \
                 a one-shot payload.\nserial:\n{stdout}\nstderr:\n{stderr}"
            );
        }
    }

    // 3. And the run must have stopped on the acceptance condition rather than
    //    grinding the ceiling, or the budget is what this gate measures.
    assert!(
        stderr.contains("both nodes printed"),
        "the run used its whole step ceiling instead of stopping on \
         {stop_on:?}\nstderr:\n{stderr}"
    );
}
