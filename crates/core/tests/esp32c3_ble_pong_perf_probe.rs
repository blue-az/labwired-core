//! Measurement probe for the two-node BLE Pong lab. **Every test here is
//! `#[ignore]`d and asserts nothing** — it is a harness for deriving numbers,
//! not a gate. Do not add assertions and call it coverage; if a property is
//! worth defending, gate it in `esp32c3_shipped_lab_batch_gate.rs` where the
//! budget file lives.
//!
//! Derived 2026-08-07 (release, M-series, `--features event-scheduler`):
//!
//! | config                    | wall (2 nodes) | note                         |
//! |---------------------------|----------------|------------------------------|
//! | 60M cyc, FF on,  1M slice | 1.46 s         | ff_ratio 0.81                |
//! | 60M cyc, FF off, 1M slice | 15.84 s        | **idle FF is worth 10.8x**   |
//! | 96M cyc, FF on,  250k     | 3.46 s         | main-thread cap              |
//! | 96M cyc, FF on,  16M      | 2.86 s         | worker cap — only **1.21x**  |
//!
//! Two conclusions that contradict the obvious guesses:
//!
//! 1. The 64x gap between `HEAVY_MAIN_THREAD_MAX_BATCH` (250k) and
//!    `HEAVY_WORKER_MAX_BATCH` (16M) buys 1.21x of engine throughput. The
//!    worker matters for keeping the UI thread free, not for speed.
//! 2. Idle FF does not bite until ~6M cycles into boot (0 skipped at 4M).
//!    A browser HUD reading `idle FF 0` on a lab that has only advanced a few
//!    million cycles is reporting health, not a bug.
//!
//! **Known confound in the tree these were taken on.** A concurrent session had
//! an uncommitted `std::env::var("LABWIRED_TIMG_TRACE")` at the top of
//! `esp32/timg.rs::sync_to`, which the C3 reaches via `RtcCalProfile`. That is
//! one env lookup per bus tick — ~300k over a 96M-cycle pair run, so <5% of
//! wall time. It does not move either conclusion, and it cannot move the 1.21×
//! at all: both cap configs ran the same batch count (149,405 vs 150,041), so
//! the overhead cancels in the ratio. Re-derive on a clean tree if the absolute
//! cycles/s ever becomes load-bearing.
#![cfg(all(feature = "event-scheduler", not(debug_assertions)))]

use labwired_config::{ChipDescriptor, SystemManifest};
use labwired_core::boot::esp32c3_rom::{
    build_rom_boot_machine, c3_rom_data_init_writes, inject_rom_regions, RomBootOpts,
};
use labwired_core::boot::esp32s3_rom::RomImages;
use labwired_core::bus::SystemBus;
use labwired_core::cpu::RiscV;
use labwired_core::memory::ProgramImage;
use labwired_core::{Arch, Bus, Cpu, DebugControl, Machine};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

const ESP_IMAGE_HEADER_LEN: usize = 24;

fn bootloader_image(flash: &[u8]) -> ProgramImage {
    let segment_count = flash[1] as usize;
    let entry = u32::from_le_bytes(flash[4..8].try_into().unwrap()) as u64;
    let mut program = ProgramImage::new(entry, Arch::RiscV);
    let mut cursor = ESP_IMAGE_HEADER_LEN;
    for _ in 0..segment_count {
        let load_addr = u32::from_le_bytes(flash[cursor..cursor + 4].try_into().unwrap()) as u64;
        let len = u32::from_le_bytes(flash[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        cursor += 8;
        program.add_segment(load_addr, flash[cursor..cursor + len].to_vec());
        cursor += len;
    }
    program
}

struct Node {
    machine: Machine<RiscV>,
    serial: Arc<Mutex<Vec<u8>>>,
}

fn build_node(flash: &[u8], idle_ff: bool) -> Node {
    let chip = ChipDescriptor::from_file(root().join("../../configs/chips/esp32c3.yaml")).unwrap();
    let manifest =
        SystemManifest::from_file(root().join("../../configs/systems/esp32c3-ble-pong.yaml"))
            .unwrap();
    let mut bus = SystemBus::from_config(&chip, &manifest).unwrap();

    let irom = std::fs::read(root().join("roms/esp32c3/esp32c3_rom.bin")).unwrap();
    let drom = std::fs::read(root().join("roms/esp32c3/esp32c3_drom.bin")).unwrap();
    assert!(inject_rom_regions(
        &mut bus,
        &RomImages {
            irom: irom.clone(),
            drom,
        },
    ));
    for (dst, bytes) in c3_rom_data_init_writes(&irom) {
        for (i, b) in bytes.iter().enumerate() {
            let _ = bus.write_u8(dst as u64 + i as u64, *b);
        }
    }

    let serial = Arc::new(Mutex::new(Vec::new()));
    bus.attach_uart_tx_sink(serial.clone(), false);

    let bootloader = bootloader_image(flash);
    let mut machine = build_rom_boot_machine(
        bus,
        flash.to_vec(),
        RomBootOpts {
            pinned_efuse_mac: None,
            usb_serial_sink: None,
        },
        |c| c,
    );
    for segment in &bootloader.segments {
        if machine.bus.flash.load_from_segment(segment)
            || machine.bus.ram.load_from_segment(segment)
            || machine
                .bus
                .extra_mem
                .iter_mut()
                .any(|m| m.load_from_segment(segment))
        {
            continue;
        }
        for (i, byte) in segment.data.iter().enumerate() {
            machine
                .bus
                .write_u8(segment.start_addr + i as u64, *byte)
                .unwrap();
        }
    }
    let sp_top = (chip.ram.base + labwired_config::parse_size(&chip.ram.size).unwrap_or(0)) as u32;
    machine.cpu.set_sp(sp_top & !0xF);
    machine.cpu.set_pc(bootloader.entry_point as u32);

    let rec = machine.bus.max_safe_tick_interval();
    machine.config.peripheral_tick_interval = rec;
    machine.bus.config.peripheral_tick_interval = rec;
    machine.config.idle_fast_forward_enabled = idle_ff;
    Node { machine, serial }
}

fn probe(label: &str, idle_ff: bool, slice: u32, budget: u64) {
    let flash = std::fs::read(root().join("tests/fixtures/esp32c3-ble-pong-flash.bin")).unwrap();
    let mut a = build_node(&flash, idle_ff);
    let mut b = build_node(&flash, idle_ff);
    a.machine.reset_step_profile();
    b.machine.reset_step_profile();

    let start = std::time::Instant::now();
    let mut fuel = 0u64;
    while fuel < budget {
        let n = slice.min((budget - fuel) as u32);
        for node in [&mut a, &mut b] {
            let _ = node.machine.run(Some(n));
        }
        fuel += u64::from(n);
    }
    let wall = start.elapsed().as_secs_f64();

    for (name, node) in [("A", &a), ("B", &b)] {
        // Batch-width attribution: who armed the wakes that ended the batches.
        let stats = node.machine.sched.stats();
        let at_now = &stats.arms_at_now_per_peripheral;
        let mut owners: Vec<(u64, u64, &str)> = stats
            .arms_per_peripheral
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(idx, n)| {
                let who = node
                    .machine
                    .bus
                    .peripherals
                    .get(idx)
                    .map(|p| p.name.as_str())
                    .unwrap_or("<out-of-range>");
                (*n, at_now.get(idx).copied().unwrap_or(0), who)
            })
            .collect();
        owners.sort_unstable_by(|x, y| y.0.cmp(&x.0));
        let total_arms: u64 = owners.iter().map(|(n, _, _)| n).sum();
        let top: Vec<String> = owners
            .iter()
            .take(6)
            .map(|(n, z, who)| format!("{who}={n}(at_now={z})"))
            .collect();

        let p = node.machine.step_profile();
        let mean_batch = p.cpu_instructions as f64 / p.cpu_batches.max(1) as f64;
        let total = node.machine.total_cycles.max(1);
        let ff = node.machine.idle_fast_forward_cycles_skipped;
        let console = String::from_utf8_lossy(&node.serial.lock().unwrap().clone()).into_owned();
        eprintln!(
            "PROBE {label} node{name} idle_ff={idle_ff} slice={slice} \
             mean_batch={mean_batch:.2} batches={} interpreted={} total_cycles={total} \
             ff_skipped={ff} ff_ratio={:.4} legacy_tick_entries={} serial_bytes={}",
            p.cpu_batches,
            p.cpu_instructions,
            ff as f64 / total as f64,
            p.legacy_tick_entries,
            console.len(),
        );
        eprintln!(
            "PROBE {label} node{name} ARMS total={total_arms} \
             arms_per_batch={:.2} top=[{}]",
            total_arms as f64 / p.cpu_batches.max(1) as f64,
            top.join(" "),
        );
        eprintln!(
            "PROBE {label} node{name} HEAP max_queued={} max_live_per_periph={} \
             ceiling_trips={} past_clamps={}",
            stats.max_queued_events,
            stats.max_live_events_per_peripheral,
            stats.live_event_ceiling_trips,
            stats.past_schedule_clamps,
        );
        let mut live: Vec<String> = stats
            .max_live_per_peripheral
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 1)
            .map(|(idx, n)| {
                let who = node
                    .machine
                    .bus
                    .peripherals
                    .get(idx)
                    .map(|p| p.name.as_str())
                    .unwrap_or("<oor>");
                (*n, who)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(n, who)| format!("{who}={n}"))
            .collect();
        live.sort();
        eprintln!("PROBE {label} node{name} LIVE_HWM [{}]", live.join(" "));
    }
    let cps = (2.0 * budget as f64) / wall;
    eprintln!(
        "PROBE {label} wall={wall:.2}s two_node_cycles_per_sec={cps:.0} rtf_pair={:.3}",
        (budget as f64 / wall) / 160e6
    );
}

#[test]
#[ignore = "measurement probe, not a gate"]
fn probe_ble_pong_idle_ff() {
    // Same total guest cycles, three batching/FF configurations.
    probe("ff_on_1M", true, 1_000_000, 60_000_000);
    probe("ff_off_1M", false, 1_000_000, 60_000_000);
    probe("ff_on_100k", true, 100_000, 60_000_000);
}

/// A long steady-state window to profile against. Boot is ~6M cycles of the
/// 400M here, so >98% of samples land in the regime that actually matters.
///
/// Run under a sampling profiler:
/// ```text
/// cargo test --release -p labwired-core --features event-scheduler \
///   --test esp32c3_ble_pong_perf_probe -- --ignored probe_ble_pong_profile &
/// sample <pid> 20 -f /tmp/pong.sample
/// ```
#[test]
#[ignore = "measurement probe, not a gate"]
fn probe_ble_pong_profile() {
    probe("profile", true, 1_000_000, 400_000_000);
}

/// Where in the boot does idle FF first bite? The browser HUD reads
/// `idle FF 0` at ~4M cycles; if native is also 0 there, that reading is
/// evidence of nothing.
#[test]
#[ignore = "measurement probe, not a gate"]
fn probe_ble_pong_ff_onset() {
    for budget in [2_000_000u64, 4_000_000, 8_000_000, 16_000_000, 32_000_000] {
        probe(&format!("onset_{}M", budget / 1_000_000), true, 250_000, budget);
    }
}

/// A1 as actually shipped: main-thread cap (250k) vs worker cap (16M).
/// Also prints serial so a slice wide enough to break the BLE election shows up
/// as a node that never leaves GUEST.
#[test]
#[ignore = "measurement probe, not a gate"]
fn probe_ble_pong_batch_cap() {
    probe("cap_250k", true, 250_000, 96_000_000);
    probe("cap_16M", true, 16_000_000, 96_000_000);
}
