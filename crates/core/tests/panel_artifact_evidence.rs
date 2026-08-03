// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! The contract, stated without reference to how the engine is built:
//!
//! > A panel that was painted reports an artifact saying so. A panel that was
//! > NOT painted reports zero — not absence.
//!
//! Absence and zero are different findings, and only one of them is checkable.
//! `labwired_verify`'s display oracle resolves a `panel` clause against the
//! artifact this seam emits; a panel that emits nothing makes
//! `{painted: true, min_ink_bytes: N}` and `min_refresh_generation` unresolvable
//! no matter how correct the firmware is. Four shipped labs — IMAX Console
//! (SH1107), Weather Station and Stats Display (SSD1680 tricolor), and any
//! UC8151D lab — were unverifiable for exactly that reason.
//!
//! Every expectation below is arithmetic on bytes THIS TEST wrote over the
//! panel's own wire protocol, never a re-read of the model's buffer. The two
//! panels that already had evidence (SSD1306, ILI9341) are pinned here too, so
//! a change that adds the missing three by disturbing the existing two cannot
//! pass.

use labwired_config::{ChipDescriptor, SystemManifest};
use labwired_core::bus::SystemBus;
use labwired_core::cpu::cortex_m::CortexM;
use labwired_core::inspect::{device_artifacts, Artifact, InspectOpts};
use labwired_core::peripherals::components::{
    Ili9341, Sh1107, Ssd1306, Ssd1680Tricolor290, Uc8151dTricolor290,
};
use labwired_core::peripherals::i2c::I2cDevice;
use labwired_core::peripherals::spi::SpiDevice;
use labwired_core::system::cortex_m::configure_cortex_m;
use labwired_core::Machine;
use std::path::PathBuf;

/// The one artifact a painted panel must produce, or a panic naming what came
/// out instead.
fn framebuffer_artifact(model: &dyn std::any::Any, id: &str) -> Artifact {
    let arts = device_artifacts(model, id, &InspectOpts::default());
    arts.into_iter()
        .find(|a| a.kind == "framebuffer")
        .unwrap_or_else(|| {
            panic!("'{id}' produced no framebuffer artifact; a panel that painted must say so")
        })
}

// ─── SH1107 (I²C OLED) ──────────────────────────────────────────────────────

/// Write `bytes` into the SH1107's GDDRAM at page 0, column 0, over the wire:
/// control byte 0x00 for commands, 0x40 for a data stream, exactly as a driver
/// does. Returns the panel.
fn painted_sh1107(bytes: &[u8]) -> Sh1107 {
    let mut oled = Sh1107::new(0x3C);
    // Command stream: display on, page 0, column 0.
    oled.start();
    oled.write(0x00);
    oled.write(0xAF); // display on
    oled.write(0xB0); // page 0
    oled.write(0x00); // column low nibble 0
    oled.write(0x10); // column high nibble 0
    oled.stop();
    // Data stream.
    oled.start();
    oled.write(0x40);
    for &b in bytes {
        oled.write(b);
    }
    oled.stop();
    oled
}

/// An SH1107 driven over its own I²C protocol reports the ink it received.
///
/// The expected counts are computed from the bytes this test sent, not read
/// back off the panel: four bytes with 1, 8, 4 and 2 bits set is 4 inked bytes
/// and 15 lit pixels.
#[test]
fn sh1107_that_painted_reports_its_ink() {
    const PATTERN: [u8; 4] = [0x01, 0xFF, 0x0F, 0x81];
    let expected_lit: usize = PATTERN.iter().map(|b| b.count_ones() as usize).sum();

    let oled = painted_sh1107(&PATTERN);
    let art = framebuffer_artifact(&oled, "oled");

    assert_eq!(art.id, "oled", "artifact is addressed by the device's id");
    assert_eq!(art.meta["w"], 128);
    assert_eq!(art.meta["h"], 128);
    assert_eq!(
        art.meta["ink_bytes"], PATTERN.len(),
        "one inked byte per non-zero byte written"
    );
    assert_eq!(art.meta["lit_pixels"], expected_lit);
    assert_eq!(art.meta["display_on"], true, "0xAF was sent");
    assert!(art.bytes.is_none(), "summary mode omits the payload");

    let full = device_artifacts(
        &oled,
        "oled",
        &InspectOpts {
            include_bytes: true,
            peripheral: None,
        },
    );
    assert_eq!(
        full[0].bytes.as_ref().map(Vec::len),
        Some(128 * 16),
        "include_bytes attaches the real GDDRAM"
    );
}

/// A panel nobody drove reports zero, not an absent artifact. "Nothing was
/// painted" is a finding and must be legible as one.
#[test]
fn unpainted_sh1107_reports_zero_rather_than_nothing() {
    let art = framebuffer_artifact(&Sh1107::new(0x3C), "oled");
    assert_eq!(art.meta["ink_bytes"], 0);
    assert_eq!(art.meta["lit_pixels"], 0);
    assert_eq!(art.meta["display_on"], false);
}

// ─── SSD1680 tricolor e-paper (SPI) ─────────────────────────────────────────

/// Stream `black`/`red` into the panel's RAM at a 1-byte-wide window and then
/// run GxEPD2's power + master-activation handshake, over the datasheet
/// command/data path.
fn painted_epaper_bytes(panel: &mut Ssd1680Tricolor290, black: &[u8], red: &[u8]) {
    panel.command_byte(0x12); // SWRESET
    panel.command_byte(0x11); // data entry mode
    panel.data_byte(0x03);
    panel.command_byte(0x44); // RAM-X window (start/8, end/8)
    panel.data_byte(0x00);
    panel.data_byte(0x00);
    panel.command_byte(0x45); // RAM-Y window
    panel.data_byte(0x00);
    panel.data_byte(0x00);
    panel.data_byte((black.len() - 1) as u8);
    panel.data_byte(0x00);
    panel.command_byte(0x4E); // RAM-X counter
    panel.data_byte(0x00);
    panel.command_byte(0x4F); // RAM-Y counter
    panel.data_byte(0x00);
    panel.data_byte(0x00);
    panel.command_byte(0x24); // black plane stream
    for &b in black {
        panel.data_byte(b);
    }
    panel.command_byte(0x4E);
    panel.data_byte(0x00);
    panel.command_byte(0x4F);
    panel.data_byte(0x00);
    panel.data_byte(0x00);
    panel.command_byte(0x26); // red plane stream
    for &b in red {
        panel.data_byte(b);
    }
    panel.command_byte(0x22); // display update control
    panel.data_byte(0xF7);
    panel.command_byte(0x20); // master activation → refresh
}

/// A tri-color e-paper that was streamed and refreshed reports BOTH planes and
/// the refresh that made them visible.
///
/// The counts are arithmetic on this test's own input: an e-paper plane is
/// erased to 0xFF (1 = white / no-red), so an inked cell is any byte that is
/// not 0xFF. Two of the three black bytes and one of the three red bytes differ
/// from the erased value.
#[test]
fn ssd1680_that_refreshed_reports_both_planes_and_the_refresh() {
    const BLACK: [u8; 3] = [0x00, 0xFF, 0xAA];
    const RED: [u8; 3] = [0xFF, 0xFF, 0x0F];
    let black_ink = BLACK.iter().filter(|&&b| b != 0xFF).count();
    let red_ink = RED.iter().filter(|&&b| b != 0xFF).count();

    let mut panel = Ssd1680Tricolor290::new("PA4");
    painted_epaper_bytes(&mut panel, &BLACK, &RED);
    let art = framebuffer_artifact(&panel, "epaper");

    assert_eq!(art.meta["w"], 128);
    assert_eq!(art.meta["h"], 296);
    assert_eq!(art.meta["black_ink_bytes"], black_ink);
    assert_eq!(art.meta["red_ink_bytes"], red_ink);
    assert_eq!(
        art.meta["refresh_generation"], 1,
        "one master activation is one refresh"
    );
    assert_eq!(art.meta["power_on"], true);
}

/// An e-paper nobody drove reports zero ink and generation zero — not absence,
/// and not a plausible-looking number.
#[test]
fn unrefreshed_ssd1680_reports_zero_rather_than_nothing() {
    let art = framebuffer_artifact(&Ssd1680Tricolor290::new("PA4"), "epaper");
    assert_eq!(art.meta["black_ink_bytes"], 0);
    assert_eq!(art.meta["red_ink_bytes"], 0);
    assert_eq!(art.meta["refresh_generation"], 0);
    assert_eq!(art.meta["power_on"], false);
}

/// The whole path, end to end: the shipped `epaper-tricolor-lab` manifest, the
/// panel reached through the SPI controller it is attached to, and the evidence
/// arriving in `Machine::inspect`'s device record — not merely out of the
/// artifact helper called directly.
#[test]
fn epaper_lab_reports_panel_evidence_through_machine_inspect() {
    const BLACK: [u8; 2] = [0x00, 0x0F];
    let yaml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/epaper-tricolor-lab/system.yaml");
    let manifest = SystemManifest::from_file(&yaml).expect("load system.yaml");
    let chip_path = yaml.parent().unwrap().join(&manifest.chip);
    let chip = ChipDescriptor::from_file(&chip_path).expect("load chip");
    let mut bus = SystemBus::from_config(&chip, &manifest).expect("build bus");

    {
        let idx = bus
            .find_peripheral_index_by_name("spi1")
            .expect("spi1 registered");
        let any = bus.peripherals[idx].dev.as_any_mut().expect("downcastable");
        let spi = any
            .downcast_mut::<labwired_core::peripherals::spi::Spi>()
            .expect("spi1 is a generic Spi");
        let panel = spi
            .attached_devices
            .iter_mut()
            .find_map(|d| {
                d.as_any_mut()
                    .and_then(|a| a.downcast_mut::<Ssd1680Tricolor290>())
            })
            .expect("SSD1680 attached to spi1");
        painted_epaper_bytes(panel, &BLACK, &[0xFF, 0xFF]);
    }

    let (cpu, _nvic) = configure_cortex_m(&mut bus);
    bus.refresh_peripheral_index();
    let machine = Machine::new(cpu, bus);
    let inspect = machine.inspect(None, &InspectOpts::default());
    let device = inspect
        .devices
        .iter()
        .find(|d| d.id == "epaper")
        .expect("the declared panel is a device");
    let art = device
        .artifacts
        .iter()
        .find(|a| a.kind == "framebuffer")
        .unwrap_or_else(|| {
            panic!("panel reports no framebuffer artifact through inspect; the display oracle has nothing to resolve against")
        });
    assert_eq!(art.id, "epaper");
    assert_eq!(art.meta["black_ink_bytes"], 2);
    assert_eq!(art.meta["refresh_generation"], 1);
}

// ─── UC8151D tricolor e-paper (SPI) ─────────────────────────────────────────

/// The UC8151D's own datasheet sequence: 0x10 opens the black (B/W) stream,
/// 0x13 the red stream, 0x12 triggers the refresh.
#[test]
fn uc8151d_that_refreshed_reports_both_planes_and_the_refresh() {
    const BLACK: [u8; 4] = [0x00, 0xFF, 0xF0, 0xFF];
    const RED: [u8; 4] = [0xFF, 0x00, 0xFF, 0xFF];
    let black_ink = BLACK.iter().filter(|&&b| b != 0xFF).count();
    let red_ink = RED.iter().filter(|&&b| b != 0xFF).count();

    let mut panel = Uc8151dTricolor290::new("PA4");
    panel.command_byte(0x04); // power on
    panel.command_byte(0x10); // black plane stream
    for &b in &BLACK {
        panel.data_byte(b);
    }
    panel.command_byte(0x13); // red plane stream
    for &b in &RED {
        panel.data_byte(b);
    }
    panel.command_byte(0x12); // display refresh

    let art = framebuffer_artifact(&panel, "epaper");
    assert_eq!(art.meta["black_ink_bytes"], black_ink);
    assert_eq!(art.meta["red_ink_bytes"], red_ink);
    assert_eq!(art.meta["refresh_generation"], 1);
    assert_eq!(art.meta["power_on"], true);
}

// ─── the two that already worked, pinned ────────────────────────────────────

/// SSD1306's payload is unchanged: same keys, same definitions.
#[test]
fn ssd1306_artifact_payload_is_unchanged() {
    const PATTERN: [u8; 3] = [0xFF, 0x01, 0x80];
    let expected_lit: usize = PATTERN.iter().map(|b| b.count_ones() as usize).sum();
    let mut oled = Ssd1306::new(0x3C);
    oled.start();
    oled.write(0x00);
    oled.write(0xAF);
    oled.stop();
    oled.start();
    oled.write(0x40);
    for &b in &PATTERN {
        oled.write(b);
    }
    oled.stop();

    let art = framebuffer_artifact(&oled, "oled");
    assert_eq!(art.meta["format"], "ssd1306_page");
    assert_eq!(art.meta["ink_bytes"], PATTERN.len());
    assert_eq!(art.meta["lit_pixels"], expected_lit);
    assert!(
        art.meta.get("w").is_some() && art.meta.get("h").is_some(),
        "dimensions stay in the payload"
    );
}

/// ILI9341's payload is unchanged, including the `painted_bytes` definition —
/// deliberately the same count the CLI's `painted bytes=` line prints, so the
/// two agree by construction rather than by coincidence.
#[test]
fn ili9341_artifact_payload_is_unchanged() {
    const PIXELS: usize = 100;
    const HI: u8 = 0x07;
    const LO: u8 = 0xE0; // RGB565 green: both bytes non-zero.
    let mut panel = Ili9341::new("PA4");
    panel.set_dc_level(false);
    panel.transfer(0x29); // DISPON
    panel.set_dc_level(false);
    panel.transfer(0x2C); // RAMWR
    for _ in 0..PIXELS {
        panel.set_dc_level(true);
        panel.transfer(HI);
        panel.set_dc_level(true);
        panel.transfer(LO);
    }

    let art = framebuffer_artifact(&panel, "tft");
    assert_eq!(art.meta["format"], "rgb565_be");
    assert_eq!(art.meta["display_on"], true);
    assert_eq!(art.meta["painted_bytes"], PIXELS * 2);
    assert_eq!(art.meta["top_colour"], "0x07E0");
    assert_eq!(art.meta["top_colour_pixels"], PIXELS);
}
