//! Inventory: why `max_safe_tick_interval` stays 1 on each shipped WASM family.
//!
//! `max_safe_tick_interval` returns [`RECOMMENDED_TICK_INTERVAL`] (512) only when
//! `legacy_walk_disabled && !iolink && !flash_models_ops && !hcsr04_forced_legacy`
//! (see `bus/policy.rs`). Under `event-scheduler`, walk deletion auto-derives
//! when every peripheral is `uses_scheduler() || !needs_legacy_walk()`.
//!
//! This test builds each family's real chip+system bus with `walk_deleted =
//! None` (auto-derive), prints the walk-forcing set and non-forcer blockers,
//! and guards the already-green C3 / F103 paths. Inventory only — no migrations.

use labwired_config::{ChipDescriptor, SystemManifest};
use labwired_core::bus::{SystemBus, RECOMMENDED_TICK_INTERVAL};
use labwired_core::peripherals::components::IolinkMaster;
use labwired_core::peripherals::flash::Flash;
use labwired_core::peripherals::uart::Uart;
use std::path::PathBuf;

fn root(rel: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../..");
    p.push(rel);
    p
}

/// Walk-forcers: peripherals that still pin the legacy walk under derivation.
#[derive(Clone)]
struct ForcerRow {
    name: String,
    needs_legacy_walk: bool,
    uses_scheduler: bool,
}

struct Inventory {
    chip: &'static str,
    walk_deletable: bool,
    legacy_walk_disabled: bool,
    max_safe: u32,
    flash_models_ops: bool,
    has_iolink_master: bool,
    hcsr04_count: usize,
    hcsr04_scheduling_disabled: bool,
    forcers: Vec<ForcerRow>,
    /// Full peripheral roster with walk/scheduler status (for the doc dump).
    peripherals: Vec<ForcerRow>,
}

fn flash_models_ops(bus: &SystemBus) -> bool {
    bus.peripherals.iter().any(|p| {
        p.dev
            .as_any()
            .and_then(|a| a.downcast_ref::<Flash>())
            .is_some_and(|f| f.models_ops())
    })
}

fn has_iolink_master(bus: &SystemBus) -> bool {
    for p in &bus.peripherals {
        let Some(any) = p.dev.as_any() else {
            continue;
        };
        let Some(uart) = any.downcast_ref::<Uart>() else {
            continue;
        };
        for stream in &uart.attached_streams {
            if let Some(sa) = stream.as_any() {
                if sa.downcast_ref::<IolinkMaster>().is_some() {
                    return true;
                }
            }
        }
    }
    false
}

fn inventory(chip: &'static str, bus: &SystemBus) -> Inventory {
    let peripherals: Vec<ForcerRow> = bus
        .peripherals
        .iter()
        .map(|p| ForcerRow {
            name: p.name.clone(),
            needs_legacy_walk: p.dev.needs_legacy_walk(),
            uses_scheduler: p.dev.uses_scheduler(),
        })
        .collect();
    let forcers: Vec<ForcerRow> = peripherals
        .iter()
        .filter(|p| p.needs_legacy_walk && !p.uses_scheduler)
        .cloned()
        .collect();
    // walk_deletable ≡ empty forcer set (same predicate as derive_walk_deletable).
    let walk_deletable = forcers.is_empty();
    Inventory {
        chip,
        walk_deletable,
        legacy_walk_disabled: bus.legacy_walk_disabled,
        max_safe: bus.max_safe_tick_interval(),
        flash_models_ops: flash_models_ops(bus),
        has_iolink_master: has_iolink_master(bus),
        hcsr04_count: bus.hcsr04.len(),
        hcsr04_scheduling_disabled: bus.hcsr04_scheduling_disabled,
        forcers,
        peripherals,
    }
}

fn print_inventory(inv: &Inventory) {
    let forcer_names: Vec<&str> = inv.forcers.iter().map(|f| f.name.as_str()).collect();
    let hcsr04_forced_legacy = inv.hcsr04_count > 0 && inv.hcsr04_scheduling_disabled;
    println!("=== {} ===", inv.chip);
    println!("  walk_deletable (empty forcers): {}", inv.walk_deletable);
    println!("  legacy_walk_disabled:           {}", inv.legacy_walk_disabled);
    println!("  flash_models_ops:               {}", inv.flash_models_ops);
    println!("  has_iolink_master:              {}", inv.has_iolink_master);
    println!(
        "  hcsr04_count / forced_legacy:    {} / {}",
        inv.hcsr04_count, hcsr04_forced_legacy
    );
    println!("  max_safe_tick_interval:         {}", inv.max_safe);
    println!("  forcers ({}): {:?}", forcer_names.len(), forcer_names);
    for f in &inv.forcers {
        println!(
            "    - {}  needs_legacy_walk={} uses_scheduler={}",
            f.name, f.needs_legacy_walk, f.uses_scheduler
        );
    }
    if inv.forcers.is_empty() {
        println!("  (no walk-forcers)");
    }
    // Compact full roster for doc capture.
    println!("  full peripheral walk/scheduler status:");
    for p in &inv.peripherals {
        let role = if p.needs_legacy_walk && !p.uses_scheduler {
            "FORCER"
        } else if p.uses_scheduler {
            "scheduler"
        } else {
            "inert"
        };
        println!(
            "    [{role:9}] {:20} needs_legacy_walk={} uses_scheduler={}",
            p.name, p.needs_legacy_walk, p.uses_scheduler
        );
    }
    println!();
}

fn load_manifest(system_rel: &str) -> SystemManifest {
    let system_path = root(system_rel);
    let mut manifest =
        SystemManifest::from_file(&system_path).unwrap_or_else(|e| {
            panic!("load system manifest {system_path:?}: {e}");
        });
    // Anchor chip path so resolve_peripheral_path finds descriptors regardless
    // of cargo-test CWD.
    let anchored = system_path
        .parent()
        .expect("system path parent")
        .join(&manifest.chip);
    manifest.chip = anchored.to_string_lossy().into_owned();
    // Inventory always auto-derives (never honor a hand walk_deleted hatch).
    manifest.walk_deleted = None;
    manifest
}

fn load_chip(chip_rel: &str) -> ChipDescriptor {
    let path = root(chip_rel);
    ChipDescriptor::from_file(&path).unwrap_or_else(|e| panic!("load chip {path:?}: {e}"))
}

fn bus_f103() -> SystemBus {
    let chip = load_chip("configs/chips/stm32f103.yaml");
    let manifest = load_manifest("examples/ssd1306-hello-lab/system.yaml");
    let mut bus = SystemBus::from_config(&chip, &manifest).expect("build f103 bus");
    let _ = labwired_core::system::cortex_m::configure_cortex_m(&mut bus);
    bus
}

fn bus_esp32c3() -> SystemBus {
    let chip = load_chip("configs/chips/esp32c3.yaml");
    let manifest = load_manifest("configs/systems/esp32c3-devkit.yaml");
    SystemBus::from_config(&chip, &manifest).expect("build esp32c3 bus")
}

fn bus_h563() -> SystemBus {
    let chip = load_chip("configs/chips/stm32h563.yaml");
    let manifest = load_manifest("configs/systems/nucleo-h563zi-demo.yaml");
    let mut bus = SystemBus::from_config(&chip, &manifest).expect("build h563 bus");
    let _ = labwired_core::system::cortex_m::configure_cortex_m(&mut bus);
    bus
}

fn bus_rp2040() -> SystemBus {
    // Opt out of in-tree bootrom so inventory sees the same peripheral set the
    // rest of the RP2040 tests assemble (bootrom is not a walk forcer anyway).
    std::env::set_var("LABWIRED_RP2040_BOOTROM", "");
    let chip = load_chip("configs/chips/rp2040.yaml");
    let manifest = load_manifest("configs/systems/rp2040-pico.yaml");
    let mut bus = SystemBus::from_config(&chip, &manifest).expect("build rp2040 bus");
    let _ = labwired_core::system::cortex_m::configure_cortex_m(&mut bus);
    bus
}

fn bus_nrf52840() -> SystemBus {
    let chip = load_chip("configs/chips/nrf52840.yaml");
    let manifest = load_manifest("configs/systems/nrf52840-dk.yaml");
    let mut bus = SystemBus::from_config(&chip, &manifest).expect("build nrf52840 bus");
    let _ = labwired_core::system::cortex_m::configure_cortex_m(&mut bus);
    bus
}

fn bus_esp32s3() -> SystemBus {
    let chip = load_chip("configs/chips/esp32s3.yaml");
    let manifest = load_manifest("configs/systems/esp32s3-zero.yaml");
    SystemBus::from_config(&chip, &manifest).expect("build esp32s3 bus")
}

/// Regression: C3 + F103 already flip walk-deletion and raise max_safe to 512
/// under `event-scheduler`. The remaining four families document forcer lists.
#[test]
fn tick_interval_inventory_all_families() {
    let rows = [
        ("stm32f103", bus_f103()),
        ("esp32c3", bus_esp32c3()),
        ("stm32h563", bus_h563()),
        ("rp2040", bus_rp2040()),
        ("nrf52840", bus_nrf52840()),
        ("esp32s3", bus_esp32s3()),
    ];

    let inventories: Vec<Inventory> = rows
        .iter()
        .map(|(name, bus)| inventory(name, bus))
        .collect();

    for inv in &inventories {
        print_inventory(inv);
    }

    // Sanity: forcer emptiness must agree with legacy_walk_disabled under
    // auto-derive (walk_deleted = None). A mismatch would mean a non-peripheral
    // latch or a hand flag leaked through.
    for inv in &inventories {
        assert_eq!(
            inv.walk_deletable, inv.legacy_walk_disabled,
            "{}: walk_deletable ({}) != legacy_walk_disabled ({}) under auto-derive",
            inv.chip, inv.walk_deletable, inv.legacy_walk_disabled
        );
    }

    #[cfg(feature = "event-scheduler")]
    {
        // Green families: max_safe must already be RECOMMENDED_TICK_INTERVAL.
        for name in ["stm32f103", "esp32c3"] {
            let inv = inventories
                .iter()
                .find(|i| i.chip == name)
                .expect("family present");
            assert!(
                inv.forcers.is_empty(),
                "{name} should have no walk-forcers, got {:?}",
                inv.forcers
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>()
            );
            assert!(
                inv.legacy_walk_disabled,
                "{name}: expected legacy_walk_disabled"
            );
            assert!(
                !inv.flash_models_ops,
                "{name}: unexpected flash_models_ops blocker"
            );
            assert!(
                !inv.has_iolink_master,
                "{name}: unexpected iolink blocker"
            );
            assert_eq!(
                inv.max_safe, RECOMMENDED_TICK_INTERVAL,
                "{name}: expected max_safe={RECOMMENDED_TICK_INTERVAL}, got {}",
                inv.max_safe
            );
        }

        // Failing families: under event-scheduler, max_safe is 1 OR already 512
        // (if someone migrated them). Either way, print full forcer lists.
        // Assert max_safe is 1 when forcers or non-forcer blockers are present;
        // allow 512 only when fully clear.
        for name in ["stm32h563", "rp2040", "nrf52840", "esp32s3"] {
            let inv = inventories
                .iter()
                .find(|i| i.chip == name)
                .expect("family present");
            let blocked = !inv.legacy_walk_disabled
                || inv.flash_models_ops
                || inv.has_iolink_master
                || (inv.hcsr04_count > 0 && inv.hcsr04_scheduling_disabled);
            if blocked {
                assert_eq!(
                    inv.max_safe, 1,
                    "{name}: blocked bus must keep max_safe=1 (got {}). \
                     forcers={:?} flash_models_ops={} iolink={} walk_off={}",
                    inv.max_safe,
                    inv.forcers
                        .iter()
                        .map(|f| f.name.as_str())
                        .collect::<Vec<_>>(),
                    inv.flash_models_ops,
                    inv.has_iolink_master,
                    !inv.legacy_walk_disabled,
                );
                // Document: either non-empty forcers OR a clear non-forcer blocker.
                assert!(
                    !inv.forcers.is_empty()
                        || inv.flash_models_ops
                        || inv.has_iolink_master
                        || (inv.hcsr04_count > 0 && inv.hcsr04_scheduling_disabled),
                    "{name}: max_safe=1 with empty forcers and no non-forcer blockers — unexpected"
                );
            } else {
                assert_eq!(
                    inv.max_safe, RECOMMENDED_TICK_INTERVAL,
                    "{name}: unblocked bus should already report max_safe={RECOMMENDED_TICK_INTERVAL}"
                );
            }
        }
    }

    #[cfg(not(feature = "event-scheduler"))]
    {
        // Featureless builds never raise the interval.
        for inv in &inventories {
            assert_eq!(
                inv.max_safe, 1,
                "{}: featureless build must keep max_safe=1",
                inv.chip
            );
        }
        let _ = &inventories;
    }
}
