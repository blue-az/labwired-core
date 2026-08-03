// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! `inspect` must see the devices the AUTHOR placed, not only the ones the
//! chip vendor put inside the die.
//!
//! On a real customer rig (Adafruit ESP32 Feather V2, a TCA9548A bus switch,
//! four VCNL4010s, an ILI9341 TFT) `Machine::inspect()` returned 52
//! peripherals, every one of them chip-internal (`iram`, `dport`, `timg0`,
//! `uart0`, …), and not one of the six devices in the manifest. Not empty:
//! absent. An external device is owned by the CONTROLLER it hangs off, never by
//! `SystemBus::peripherals`, so the walk over the peripheral list could not
//! reach it however hard it looked.
//!
//! These tests pin the two halves of the fix independently of the engine's own
//! bookkeeping: the topology (a device behind a bus switch is still a device)
//! and the evidence (a panel that was painted says so in structured form).

use labwired_config::SystemManifest;
use labwired_core::bus::SystemBus;
use labwired_core::inspect::{DeviceInspect, InspectOpts};
use labwired_core::peripherals::spi::SpiDevice;
use labwired_core::Machine;

/// The bay-occupancy rig's topology, verbatim from
/// `core/examples/esp32-bay-occupancy/system.yaml`. Four VCNL4010s cannot share
/// a bus (0x13 is fixed in silicon with no strap pin), which is the entire
/// reason the switch is there.
const BAY_OCCUPANCY: &str = r#"
name: ryan-bay-occupancy
chip: esp32
external_devices:
  - id: mux
    type: tca9548a
    connection: i2c0
    config:
      i2c_address: 0x70
  - id: bay0
    type: vcnl4010
    connection: mux
    channel: 0
    config:
      i2c_address: 0x13
  - id: bay1
    type: vcnl4010
    connection: mux
    channel: 1
    config:
      i2c_address: 0x13
  - id: bay2
    type: vcnl4010
    connection: mux
    channel: 2
    config:
      i2c_address: 0x13
  - id: bay3
    type: vcnl4010
    connection: mux
    channel: 3
    config:
      i2c_address: 0x13
  - id: tft
    type: ili9341
    connection: spi3
    config:
      cs_pin: "GPIO15"
      dc_pin: "GPIO33"
"#;

/// Assemble the rig with no firmware. The topology is a property of the
/// manifest, not of anything a program does at runtime, so nothing needs to run
/// for the devices to be there — and a test that had to boot Arduino first
/// would be measuring the boot, not the wiring.
fn bay_occupancy_machine() -> Machine<labwired_core::cpu::xtensa_lx7::XtensaLx7> {
    let manifest: SystemManifest = serde_yaml::from_str(BAY_OCCUPANCY).expect("parse manifest");
    let mut bus = SystemBus::new();
    let cpu = labwired_core::system::xtensa::configure_xtensa_esp32(&mut bus);
    labwired_core::system::xtensa::attach_esp32_external_devices(&mut bus, &manifest)
        .expect("attach external devices");
    bus.refresh_peripheral_index();
    Machine::new(cpu, bus)
}

fn device<'a>(devices: &'a [DeviceInspect], id: &str) -> &'a DeviceInspect {
    devices
        .iter()
        .find(|d| d.id == id)
        .unwrap_or_else(|| panic!("no device '{id}' in inspect; got {:?}", ids(devices)))
}

fn ids(devices: &[DeviceInspect]) -> Vec<&str> {
    devices.iter().map(|d| d.id.as_str()).collect()
}

/// Every device the manifest declares reaches `inspect`, with the wiring the
/// author wrote — including the four sensors behind the bus switch, which the
/// switch owns and the controller therefore never sees directly.
#[test]
fn manifest_external_devices_are_visible_in_inspect() {
    let machine = bay_occupancy_machine();
    let inspect = machine.inspect(None, &InspectOpts::default());

    let declared: Vec<&str> = inspect
        .devices
        .iter()
        .filter(|d| d.declared)
        .map(|d| d.id.as_str())
        .collect();
    assert_eq!(
        declared,
        vec!["tft", "mux", "bay0", "bay1", "bay2", "bay3"],
        "all six declared devices present"
    );

    let mux = device(&inspect.devices, "mux");
    assert_eq!(mux.device_type.as_deref(), Some("tca9548a"));
    assert_eq!(mux.attachment.transport, "i2c");
    assert_eq!(mux.attachment.bus.as_deref(), Some("i2c0"));
    assert_eq!(mux.attachment.address, Some(0x70));
    assert_eq!(
        mux.attachment.channel, None,
        "the switch is not behind itself"
    );

    for (n, id) in ["bay0", "bay1", "bay2", "bay3"].iter().enumerate() {
        let bay = device(&inspect.devices, id);
        assert_eq!(bay.device_type.as_deref(), Some("vcnl4010"));
        assert_eq!(
            bay.attachment.bus.as_deref(),
            Some("i2c0"),
            "{id} reports its controller"
        );
        assert_eq!(
            bay.attachment.address,
            Some(0x13),
            "{id} keeps the fixed address that made the switch necessary"
        );
        assert_eq!(
            bay.attachment.mux_address,
            Some(0x70),
            "{id} is behind the switch"
        );
        assert_eq!(
            bay.attachment.channel,
            Some(n as u8),
            "{id} on its own channel"
        );
    }

    let tft = device(&inspect.devices, "tft");
    assert_eq!(tft.device_type.as_deref(), Some("ili9341"));
    assert_eq!(tft.attachment.transport, "spi");
    assert_eq!(tft.attachment.bus.as_deref(), Some("spi3"));
    assert_eq!(tft.attachment.cs_pin.as_deref(), Some("GPIO15"));
    assert_eq!(tft.attachment.address, None, "SPI has no bus address");
}

/// The peripheral list is untouched: `devices` is an addition, not a
/// re-homing. A consumer reading `peripherals` sees exactly what it saw before.
#[test]
fn external_devices_do_not_appear_as_peripherals() {
    let machine = bay_occupancy_machine();
    let inspect = machine.inspect(None, &InspectOpts::default());
    for id in ["mux", "bay0", "bay1", "bay2", "bay3", "tft"] {
        assert!(
            !inspect.peripherals.iter().any(|p| p.name == id),
            "'{id}' is a device, not an MMIO peripheral; it must not be faked into the \
             peripheral list with an invented base address"
        );
    }
    assert!(
        inspect.peripherals.iter().any(|p| p.name == "i2c0"),
        "the controllers themselves are still peripherals"
    );
}

/// A live device with no manifest declaration is reported as itself, not handed
/// someone else's name.
///
/// Classic ESP32 hardwires a BMP280 at 0x76 onto i2c0 in
/// `configure_xtensa_esp32` — real, on the bus, and declared by nobody. It must
/// show up (it answers on a bus the firmware can address) and it must NOT be
/// labelled `mux` merely for being the first thing the walk found.
#[test]
fn undeclared_device_is_reported_without_a_fabricated_identity() {
    let machine = bay_occupancy_machine();
    let inspect = machine.inspect(None, &InspectOpts::default());
    let board = device(&inspect.devices, "i2c0@0x76");
    assert!(!board.declared, "no manifest entry claims this device");
    assert_eq!(
        board.device_type, None,
        "type is never guessed from the Rust model"
    );
    assert_eq!(board.attachment.address, Some(0x76));
}

/// Painted-panel evidence comes out of `inspect` as structured data.
///
/// `painted_bytes` is checked against a count derived OUTSIDE the engine: the
/// test writes a known number of pixels over the wire in a known colour whose
/// RGB565 encoding has two non-zero bytes, so the expected byte count is
/// arithmetic on the test's own input, not a re-read of the model's buffer.
#[test]
fn ili9341_reports_painted_bytes_matching_what_was_drawn() {
    const PIXELS: usize = 300;
    // RGB565 green: 0x07E0. Both bytes non-zero => 2 painted bytes per pixel.
    const HI: u8 = 0x07;
    const LO: u8 = 0xE0;

    let manifest: SystemManifest = serde_yaml::from_str(
        r#"
name: tft-only
chip: esp32
external_devices:
  - id: screen
    type: ili9341
    connection: spi3
    config:
      cs_pin: "GPIO15"
      dc_pin: "GPIO33"
"#,
    )
    .expect("parse manifest");
    let mut bus = SystemBus::new();
    let cpu = labwired_core::system::xtensa::configure_xtensa_esp32(&mut bus);
    labwired_core::system::xtensa::attach_esp32_external_devices(&mut bus, &manifest)
        .expect("attach TFT");

    // Drive the panel over its own wire protocol: DISPON, then a RAMWR pixel
    // stream. Reaching into the model's framebuffer directly would prove
    // nothing about whether a driver's bytes ever land there.
    {
        let idx = bus
            .find_peripheral_index_by_name("spi3")
            .expect("spi3 registered");
        let any = bus.peripherals[idx].dev.as_any_mut().expect("downcastable");
        let spi = any
            .downcast_mut::<labwired_core::peripherals::esp32::spi::Esp32Spi>()
            .expect("spi3 is an Esp32Spi");
        let panel = &mut spi.attached_devices[0];
        panel.cs_select();
        let cmd = |p: &mut Box<dyn SpiDevice>, b: u8| {
            p.set_dc_level(false);
            p.transfer(b);
        };
        let data = |p: &mut Box<dyn SpiDevice>, b: u8| {
            p.set_dc_level(true);
            p.transfer(b);
        };
        cmd(panel, 0x29); // DISPON
        cmd(panel, 0x2C); // RAMWR
        for _ in 0..PIXELS {
            data(panel, HI);
            data(panel, LO);
        }
        panel.cs_release();
    }
    bus.refresh_peripheral_index();
    let machine = Machine::new(cpu, bus);

    let inspect = machine.inspect(None, &InspectOpts::default());
    let screen = device(&inspect.devices, "screen");
    let fb = screen
        .artifacts
        .iter()
        .find(|a| a.kind == "framebuffer")
        .expect("panel reports a framebuffer artifact");

    assert_eq!(fb.id, "screen", "artifact is addressed by the device's id");
    assert_eq!(fb.meta["format"], "rgb565_be");
    assert_eq!(fb.meta["w"], 240);
    assert_eq!(fb.meta["h"], 320);
    assert_eq!(fb.meta["display_on"], true, "DISPON was sent");
    assert_eq!(
        fb.meta["painted_bytes"],
        PIXELS * 2,
        "one painted byte per non-zero byte the driver wrote"
    );
    assert_eq!(fb.meta["top_colour"], "0x07E0");
    assert_eq!(fb.meta["top_colour_pixels"], PIXELS);
    assert!(fb.bytes.is_none(), "summary mode omits the 153 KB payload");

    // include_bytes attaches the real buffer.
    let full = machine.inspect(
        None,
        &InspectOpts {
            include_bytes: true,
            peripheral: None,
        },
    );
    let fb = device(&full.devices, "screen")
        .artifacts
        .iter()
        .find(|a| a.kind == "framebuffer")
        .expect("framebuffer artifact")
        .clone();
    assert_eq!(fb.bytes.as_ref().map(Vec::len), Some(240 * 320 * 2));
}

/// A panel that was never driven reports zero, not an absent artifact and not a
/// plausible-looking number. "Nothing was painted" is a finding; it must be
/// legible as one.
#[test]
fn unpainted_panel_reports_zero_rather_than_nothing() {
    let machine = bay_occupancy_machine();
    let inspect = machine.inspect(None, &InspectOpts::default());
    let fb = device(&inspect.devices, "tft")
        .artifacts
        .iter()
        .find(|a| a.kind == "framebuffer")
        .expect("an unpainted panel still reports its framebuffer");
    assert_eq!(fb.meta["painted_bytes"], 0);
    assert_eq!(fb.meta["display_on"], false);
    assert_eq!(fb.meta["top_colour"], serde_json::Value::Null);
}

/// `SystemBus::record_external_devices` is the one home for the manifest
/// declarations inspect names devices from, and it has exactly two callers:
/// `from_config` (every chip that loads peripherals from a descriptor) and the
/// classic-ESP32 glue (which builds its peripheral bank in Rust instead).
///
/// A third attach path that forgets to call it would not fail loudly — its
/// devices would still attach and still simulate, and would simply inspect as
/// anonymous `i2c0@0x70` entries. That is precisely the kind of silent,
/// partial regression this file exists to prevent, so the fork is caught here
/// at the source level rather than by whichever rig happens to notice.
#[test]
fn record_external_devices_has_one_home() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut callers = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source");
            for line in text.lines() {
                if line.contains(".record_external_devices(") {
                    let rel = path
                        .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned();
                    callers.push(rel);
                }
            }
        }
    }
    callers.sort();
    assert_eq!(
        callers,
        vec![
            "src/bus/from_config.rs".to_string(),
            "src/system/xtensa/esp32.rs".to_string(),
        ],
        "a new external-device attach path must record its declarations too, \
         or its devices inspect as anonymous addresses"
    );
}
