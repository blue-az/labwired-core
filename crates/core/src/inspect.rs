// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! Universal peripheral inspection interface (Slice 1 — core machinery).
//!
//! A uniform, schema-driven way to read any peripheral's decoded
//! register/artifact state. The design intent (see `debug-inspect-proposal.md`):
//!
//! * **Snapshot semantics only** — inspect reads the post-run / paused machine
//!   state; it is not a live stepped debugger.
//! * **Side-effect-free decode** — [`default_inspect`] reads register words via
//!   [`Peripheral::peek`], never `read()`, so inspecting a read-to-clear
//!   register never perturbs it.
//! * **Honest gaps** — [`crate::Machine::peek`] returns an explicit
//!   [`PeekByte::Unmapped`] marker for unmodeled address space instead of silent
//!   zeros, so unmapped regions never look like real data. The same rule holds
//!   one level up: [`RegisterView::value`] is `None` when the peripheral's model
//!   did not answer the probe, so a NAMED register with nothing behind it is
//!   never reported as containing zero.
//! * **Naming is not modelling** — a schema contributes names, offsets and bit
//!   slices, never behaviour. Both [`inspect_with_schema`] and
//!   [`RegisterView::value`] exist to keep that visible rather than merely
//!   documented.
//! * **External devices are not peripherals** — an I²C sensor or SPI panel is
//!   owned by its controller, not by the bus, so it is enumerated separately in
//!   [`MachineInspect::devices`] instead of being given an invented base
//!   address.
//! * **Summary mode by default** — big artifact payloads (framebuffers) are
//!   omitted unless [`InspectOpts::include_bytes`] is set; a cheap
//!   `meta.generation` hash lets callers skip re-pulling unchanged buffers.
//!
//! The highest-leverage piece is [`default_inspect`]: any peripheral that
//! returns a schema from [`Peripheral::describe_registers`] (every declarative
//! `GenericPeripheral` — the whole ESP32-C3/S3 register wall) gets named,
//! field-decoded registers for zero bespoke code.

use crate::Peripheral;
use serde::{Deserialize, Serialize};

/// One field within a register schema: a named bit slice `[msb, lsb]`.
///
/// Mirrors [`labwired_config::FieldDescriptor`] but in the inspect vocabulary,
/// decoupled from the on-disk config format.
#[derive(Debug, Clone, Serialize)]
pub struct FieldSchema {
    pub name: String,
    /// `[msb, lsb]`, inclusive.
    pub bits: [u8; 2],
}

/// The register-layout schema a peripheral advertises for decoding.
///
/// Mirrors [`labwired_config::RegisterDescriptor`]. Declarative peripherals
/// return this straight from their descriptor; native peripherals may return a
/// static map or `None` (then inspect yields registers with no schema).
#[derive(Debug, Clone, Serialize)]
pub struct RegisterSchema {
    pub name: String,
    pub offset: u64,
    /// Bit width: 8, 16, or 32.
    pub size: u8,
    /// `"rw"` | `"ro"` | `"wo"`.
    pub access: &'static str,
    pub fields: Vec<FieldSchema>,
}

/// A decoded field value: the schema slice plus its extracted value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldView {
    pub name: String,
    /// `[msb, lsb]`, inclusive.
    pub bits: [u8; 2],
    pub value: u32,
}

/// One decoded register: the live raw word plus schema-decoded fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterView {
    pub name: String,
    pub offset: u64,
    /// Bit width: 8, 16, or 32.
    pub size: u8,
    /// Live raw word, read side-effect-free via [`Peripheral::peek`] — or
    /// `None` (JSON `null`) when the model **did not answer the probe**.
    ///
    /// [`Peripheral::peek`] defaults to `None` and only a handful of models
    /// override it, so most named registers have no value behind them. This
    /// used to substitute `0`, which a debugger, the web UI and an agent all
    /// read as "this register genuinely contains zero" — a fabricated reading,
    /// and worst of all for a [`crate::peripherals::stub::StubPeripheral`],
    /// whose registers are then confidently named AND permanently wrong.
    ///
    /// `null` is the same choice [`PeekByte::Unmapped`] makes for address
    /// space, for the same reason: absence of an answer must not be
    /// representable as an answer. A register is `Some` only when EVERY byte of
    /// it came back from the model.
    #[serde(default)]
    pub value: Option<u32>,
    /// Decoded via `bit_range`. Empty when the register carries no field
    /// schema — and also when `value` is `None`, because a field extracted from
    /// a word nobody supplied would be the same fabrication one level down.
    pub fields: Vec<FieldView>,
    /// `"rw"` | `"ro"` | `"wo"`.
    pub access: String,
}

/// A typed non-register artifact (framebuffer, uart ring, bus trace, pins …).
///
/// Large payloads live in `bytes`, which is omitted in summary mode; callers
/// use `meta.generation` (a cheap content hash) to detect changes without
/// re-pulling the bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// `"framebuffer"` | `"uart"` | `"bus_trace"` | `"pins"` | …
    pub kind: String,
    /// Device / stream id.
    pub id: String,
    /// `{ width, height, format, generation, … }`.
    pub meta: serde_json::Value,
    /// Large payload; present only when `include_bytes` was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
}

/// A single peripheral's decoded state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeripheralInspect {
    pub name: String,
    /// Coarse kind: `"declarative"` | `"native"` | `"i2c"` | …
    pub kind: String,
    pub base: u64,
    pub registers: Vec<RegisterView>,
    pub artifacts: Vec<Artifact>,
}

/// How an external (off-chip) device is wired into the machine.
///
/// Everything here is *placement*, not fidelity: it says where the device hangs
/// and by what address, never that any particular register of it is modeled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceAttachment {
    /// `"i2c"` | `"spi"`.
    pub transport: String,
    /// Controller peripheral the device hangs off (`"i2c0"`, `"spi3"`), i.e. a
    /// name that appears in [`MachineInspect::peripherals`].
    pub bus: String,
    /// 7-bit I²C address the model answers to, when the transport has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<u8>,
    /// Chip-select pin label, for SPI devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cs_pin: Option<String>,
    /// Address of the I²C bus switch this device sits behind, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mux_address: Option<u8>,
    /// Downstream channel of that bus switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
}

/// One external device's inspect record.
///
/// External devices are NOT peripherals: they have no MMIO window and no base
/// address, they are owned by the controller they hang off. They therefore get
/// their own list rather than being faked into
/// [`MachineInspect::peripherals`] with a made-up base.
///
/// A record here means "this model is on the bus" and nothing more. `registers`
/// is deliberately absent: an I²C/SPI device has no memory-mapped register
/// window this engine can `peek`, and inventing one would be exactly the
/// "naming a register makes it modeled" cheat [`inspect_with_schema`] warns
/// about. Evidence a device genuinely produced (a painted framebuffer) shows up
/// in `artifacts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInspect {
    /// The `id:` its manifest entry declared. Synthesized from the attachment
    /// (`"i2c0@0x70"`) when the live model cannot be matched to a declaration —
    /// e.g. a device a test attached programmatically.
    pub id: String,
    /// The manifest `type:` (`"tca9548a"`, `"ili9341"`), or `None` when no
    /// declaration matched. Never guessed from the Rust type.
    #[serde(default)]
    pub device_type: Option<String>,
    /// `true` when `id`/`device_type` came from a manifest declaration that was
    /// matched to this live model; `false` when they were synthesized.
    #[serde(default)]
    pub declared: bool,
    pub attachment: DeviceAttachment,
    pub artifacts: Vec<Artifact>,
}

/// The whole machine's decoded state (or a single filtered peripheral).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineInspect {
    pub peripherals: Vec<PeripheralInspect>,
    /// External (off-chip) devices attached to the controllers above. Added
    /// after the original shape shipped, hence `serde(default)`: an inspect
    /// blob written before this field existed still deserializes.
    #[serde(default)]
    pub devices: Vec<DeviceInspect>,
}

/// Options controlling an inspect walk.
#[derive(Debug, Clone, Default)]
pub struct InspectOpts {
    /// When `true`, artifacts carry their full byte payload; otherwise summary
    /// mode (metadata + generation hash only).
    pub include_bytes: bool,
    /// Restrict the walk to a single peripheral by name. `None` = all.
    pub peripheral: Option<String>,
}

/// One byte of a [`crate::Machine::peek`] read.
///
/// Modeled space yields [`PeekByte::Mapped`]; a gap (no memory region and no
/// peripheral window covers the address) yields [`PeekByte::Unmapped`] — never a
/// silent zero, so unmodeled space cannot be mistaken for real data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeekByte {
    Mapped(u8),
    Unmapped,
}

/// The result of a [`crate::Machine::peek`]: `len` bytes starting at `addr`,
/// each carrying an explicit mapped/unmapped marker.
#[derive(Debug, Clone, Serialize)]
pub struct PeekResult {
    pub addr: u64,
    pub bytes: Vec<PeekByte>,
}

impl PeekResult {
    /// Collapse to raw bytes, substituting `0` for unmapped positions. Used by
    /// the wasm raw escape hatch, which returns a plain byte buffer; honest
    /// callers use [`PeekResult::bytes`] directly.
    pub fn to_lossy_bytes(&self) -> Vec<u8> {
        self.bytes
            .iter()
            .map(|b| match b {
                PeekByte::Mapped(v) => *v,
                PeekByte::Unmapped => 0,
            })
            .collect()
    }
}

/// Extract the value of a `[msb, lsb]` (inclusive) bit slice from `word`.
fn extract_field(word: u32, bits: [u8; 2]) -> u32 {
    let msb = bits[0].min(31);
    let lsb = bits[1].min(msb);
    let width = msb - lsb + 1;
    let mask = if width >= 32 {
        u32::MAX
    } else {
        (1u32 << width) - 1
    };
    (word >> lsb) & mask
}

/// Assemble a little-endian register word of `size` bits from side-effect-free
/// [`Peripheral::peek`] byte reads.
///
/// `None` if ANY byte of the register went unanswered. Partial is not a useful
/// middle ground: half a word from the model and half invented is a word that
/// looks exactly like data and is not, so the whole register is reported as
/// unreadable instead. This used to `unwrap_or(0)` each byte, which is where
/// every named-but-unmodeled register got its confident `0x00000000`.
fn peek_word<P: Peripheral + ?Sized>(p: &P, offset: u64, size: u8) -> Option<u32> {
    let n = (size / 8).max(1) as u64;
    let mut word: u32 = 0;
    for i in 0..n {
        let byte = p.peek(offset + i)? as u32;
        word |= byte << (8 * i);
    }
    Some(word)
}

/// Decode a register's named fields, or nothing at all when the model did not
/// supply the word. Shared by [`default_inspect`] and [`inspect_with_schema`]
/// so the two cannot disagree about what an unreadable register looks like.
fn decode_fields(value: Option<u32>, fields: &[FieldSchema]) -> Vec<FieldView> {
    let Some(value) = value else {
        return Vec::new();
    };
    fields
        .iter()
        .map(|f| FieldView {
            name: f.name.clone(),
            bits: f.bits,
            value: extract_field(value, f.bits),
        })
        .collect()
}

/// Generic peripheral inspection: walk the register schema, decode each word
/// (side-effect-free via `peek`) and its named fields.
///
/// This is the default body of [`Peripheral::inspect`]. Peripherals that expose
/// non-register artifacts (framebuffers, traces) override `inspect`, typically
/// by calling `default_inspect` and pushing artifacts onto the result.
///
/// Generic over `?Sized` so the trait default can pass `self` (a `&dyn
/// Peripheral`) through without a `Sized` bound — keeping `inspect`
/// object-safe.
pub fn default_inspect<P: Peripheral + ?Sized>(
    p: &P,
    base: u64,
    name: &str,
    _opts: &InspectOpts,
) -> PeripheralInspect {
    let schema = p.describe_registers();
    let kind = if schema.is_some() {
        "declarative"
    } else {
        "native"
    };

    let mut registers = Vec::new();
    if let Some(schema) = schema {
        for reg in schema {
            let value = peek_word(p, reg.offset, reg.size);
            let fields = decode_fields(value, &reg.fields);
            registers.push(RegisterView {
                name: reg.name,
                offset: reg.offset,
                size: reg.size,
                value,
                fields,
                access: reg.access.to_string(),
            });
        }
    }

    PeripheralInspect {
        name: name.to_string(),
        kind: kind.to_string(),
        base,
        registers,
        artifacts: Vec::new(),
    }
}

/// Translate an on-disk [`labwired_config::PeripheralDescriptor`] into the
/// inspect vocabulary.
///
/// One conversion, two callers: declarative peripherals describe themselves
/// with it ([`crate::peripherals::declarative::GenericPeripheral::describe_registers`]),
/// and native peripherals borrow it via a chip's `debug_schema` (see
/// [`inspect_with_schema`]). Keeping it here stops the two from drifting into
/// two slightly different ideas of what a register looks like.
pub fn schema_from_descriptor(
    descriptor: &labwired_config::PeripheralDescriptor,
) -> Vec<RegisterSchema> {
    descriptor
        .registers
        .iter()
        .map(|reg| RegisterSchema {
            name: reg.id.clone(),
            offset: reg.address_offset,
            size: reg.size,
            access: match reg.access {
                labwired_config::Access::ReadWrite => "rw",
                labwired_config::Access::ReadOnly => "ro",
                labwired_config::Access::WriteOnly => "wo",
            },
            fields: reg
                .fields
                .iter()
                .map(|f| FieldSchema {
                    name: f.name.clone(),
                    bits: f.bit_range,
                })
                .collect(),
        })
        .collect()
}

/// Decode a peripheral's live state against an EXTERNALLY supplied schema.
///
/// This exists for **native** peripherals — the ones that model behaviour in
/// hand-written Rust and so advertise no [`Peripheral::describe_registers`] of
/// their own. Before this, every such peripheral inspected as
/// `kind: "native", registers: []`, which in a debugger reads as "this
/// peripheral has no registers" when the truth is "nobody told the debugger
/// their names". On nRF52840 that was all 52 of them.
///
/// **This adds no fidelity, and must never be counted as any.** The schema
/// contributes names, offsets and bit slices — nothing else. Every VALUE still
/// comes from the peripheral's own model, read through side-effect-free `peek`,
/// exactly as `default_inspect` would. A register the model does not implement
/// reads back whatever the model returns for that offset; naming it does not
/// make it modeled.
///
/// `kind` stays `"native"` for that reason: the caller can still tell a
/// hand-written peripheral from a declarative one.
pub fn inspect_with_schema<P: Peripheral + ?Sized>(
    p: &P,
    base: u64,
    name: &str,
    schema: &[RegisterSchema],
) -> PeripheralInspect {
    let registers = schema
        .iter()
        .map(|reg| {
            let value = peek_word(p, reg.offset, reg.size);
            RegisterView {
                name: reg.name.clone(),
                offset: reg.offset,
                size: reg.size,
                value,
                fields: decode_fields(value, &reg.fields),
                access: reg.access.to_string(),
            }
        })
        .collect();

    PeripheralInspect {
        name: name.to_string(),
        kind: "native".to_string(),
        base,
        registers,
        artifacts: Vec::new(),
    }
}

/// A live external device found hanging off a controller during the inspect
/// walk — the borrowed, pre-identity form of [`DeviceInspect`].
///
/// Controllers hand these out from
/// [`Peripheral::for_each_attached_device`](crate::Peripheral::for_each_attached_device).
/// They carry only what the controller genuinely knows (transport, address,
/// chip-select, bus-switch position) plus a borrow of the model itself; the
/// manifest identity is joined on afterwards by
/// [`crate::Machine::inspect`], which is the only place that has the
/// declarations.
pub struct AttachedDeviceRef<'a> {
    /// `"i2c"` | `"spi"`.
    pub transport: &'static str,
    pub address: Option<u8>,
    pub cs_pin: Option<&'a str>,
    /// Set when this device sits behind an I²C bus switch.
    pub mux_address: Option<u8>,
    pub channel: Option<u8>,
    /// The model, for artifact extraction. `None` when the model does not
    /// expose [`std::any::Any`] — then it can be listed but not read.
    pub model: Option<&'a dyn std::any::Any>,
}

/// Visit one I²C slave, then everything wired behind it if it is a bus switch.
///
/// Every I²C controller's `for_each_attached_device` funnels through this, so
/// the "a mux owns its children, the controller only sees the mux" topology is
/// unfolded in ONE place. A controller that walked its slave list itself would
/// subtract every device behind a switch — which is precisely why the four
/// VCNL4010s on the bay-occupancy rig were invisible.
pub fn visit_i2c_device(
    dev: &dyn crate::peripherals::i2c::I2cDevice,
    f: &mut dyn FnMut(AttachedDeviceRef<'_>),
) {
    let model = dev.as_any();
    f(AttachedDeviceRef {
        transport: "i2c",
        address: Some(dev.address()),
        cs_pin: None,
        mux_address: None,
        channel: None,
        model,
    });
    let Some(mux) =
        model.and_then(|a| a.downcast_ref::<crate::peripherals::components::tca9548a::Tca9548a>())
    else {
        return;
    };
    let mux_address = crate::peripherals::i2c::I2cDevice::address(mux);
    for ch in 0..crate::peripherals::components::tca9548a::TCA9548A_CHANNELS as u8 {
        for child in mux.channel_devices(ch) {
            f(AttachedDeviceRef {
                transport: "i2c",
                address: Some(child.address()),
                cs_pin: None,
                mux_address: Some(mux_address),
                channel: Some(ch),
                model: child.as_any(),
            });
        }
    }
}

/// Visit one SPI device. The SPI counterpart of [`visit_i2c_device`]; SPI has
/// no bus-switch analogue in this engine, so there is nothing to unfold.
pub fn visit_spi_device(
    dev: &dyn crate::peripherals::spi::SpiDevice,
    f: &mut dyn FnMut(AttachedDeviceRef<'_>),
) {
    f(AttachedDeviceRef {
        transport: "spi",
        address: None,
        cs_pin: Some(dev.cs_pin()),
        mux_address: None,
        channel: None,
        model: dev.as_any(),
    });
}

/// The ONE place that turns an external device model into inspect artifacts.
///
/// Before this, panel evidence existed in two disconnected forms: an
/// `Artifact` that only the STM32 and ESP32-C3 I²C controllers emitted, and
/// only for an SSD1306; and `eprintln!` lines in the CLI that a shell script
/// scraped with `sed`. A panel on any other controller — the ILI9341 on
/// classic-ESP32 `spi3`, say — produced structured evidence for neither.
///
/// **This adds no fidelity.** Every number below is read straight off the
/// model's own buffer; nothing is synthesized, and a panel whose driver never
/// painted reports zero rather than something plausible. Models with no
/// arm here simply return no artifacts, which is honest: absent means
/// "this engine has nothing to show", never "the screen was blank".
pub fn device_artifacts(model: &dyn std::any::Any, id: &str, opts: &InspectOpts) -> Vec<Artifact> {
    use crate::peripherals::components::{Ili9341, Ssd1306};

    let mut out = Vec::new();
    let bytes_of = |buf: &[u8]| {
        if opts.include_bytes {
            Some(buf.to_vec())
        } else {
            None
        }
    };

    if let Some(oled) = model.downcast_ref::<Ssd1306>() {
        let fb = oled.framebuffer();
        out.push(Artifact {
            kind: "framebuffer".to_string(),
            id: id.to_string(),
            meta: serde_json::json!({
                "w": oled.width(),
                "h": oled.height(),
                "format": "ssd1306_page",
                "generation": artifact_generation(fb),
                "ink_bytes": oled.ink_bytes(),
                "lit_pixels": oled.lit_pixels(),
            }),
            bytes: bytes_of(fb),
        });
    } else if let Some(panel) = model.downcast_ref::<Ili9341>() {
        let fb = panel.framebuffer();
        // An RGB565 TFT has no e-paper "refresh": frame memory IS the screen,
        // so the evidence is DISPON plus how much of the buffer the firmware
        // actually wrote. `painted_bytes` counts non-zero bytes — the SAME
        // definition the CLI's `painted bytes=` line prints, so the two agree
        // by construction instead of by coincidence.
        let painted = fb.iter().filter(|&&b| b != 0x00).count();
        let (w, h) = panel.dimensions();
        // The most common non-black pixel: says WHAT was drawn, not merely
        // that something was. "6352 bytes changed" cannot be checked against a
        // photo of real silicon; "top colour 0x07E0" (RGB565 green) can.
        let mut counts: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
        for px in fb.chunks_exact(2) {
            let v = u16::from_be_bytes([px[0], px[1]]);
            if v != 0 {
                *counts.entry(v).or_default() += 1;
            }
        }
        let top = counts.iter().max_by_key(|&(_, n)| *n);
        out.push(Artifact {
            kind: "framebuffer".to_string(),
            id: id.to_string(),
            meta: serde_json::json!({
                "w": w,
                "h": h,
                "format": "rgb565_be",
                "generation": artifact_generation(fb),
                "display_on": panel.display_on(),
                "painted_bytes": painted,
                "total_bytes": fb.len(),
                "top_colour": top.map(|(v, _)| format!("0x{v:04X}")),
                "top_colour_pixels": top.map(|(_, n)| *n),
            }),
            bytes: bytes_of(fb),
        });
    }
    out
}

/// FNV-1a hash of a byte buffer, used as a cheap `meta.generation` so callers
/// can detect an unchanged artifact without re-pulling its bytes.
pub fn artifact_generation(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_field_slices_bits() {
        // 0b1101_10 -> value 0b110110 = 54
        let word = 0b0000_0000_0000_0000_0000_0000_0011_0110;
        assert_eq!(extract_field(word, [1, 0]), 0b10);
        assert_eq!(extract_field(word, [2, 2]), 0b1);
        assert_eq!(extract_field(word, [5, 0]), 0b110110);
        assert_eq!(extract_field(word, [31, 0]), word);
    }

    #[test]
    fn generation_changes_with_bytes() {
        assert_ne!(
            artifact_generation(&[0, 0, 0]),
            artifact_generation(&[0, 1, 0])
        );
        assert_eq!(
            artifact_generation(&[1, 2, 3]),
            artifact_generation(&[1, 2, 3])
        );
    }

    /// A model that answers `peek` and a model that does not must not produce
    /// the same JSON.
    ///
    /// This is the whole point of the nullable value. `Peripheral::peek`
    /// defaults to `None`, so most native peripherals never answer — and a
    /// schema names their registers regardless. Reporting `0` for those made
    /// "nobody asked the model" indistinguishable from "the model says zero",
    /// which is precisely the confusion this module's own doc comment says it
    /// exists to prevent. The two peripherals below carry the SAME schema and
    /// differ only in whether they implement `peek`.
    #[test]
    fn unanswered_probe_is_null_not_zero() {
        use crate::{Peripheral, SimResult};

        fn schema() -> Vec<RegisterSchema> {
            vec![RegisterSchema {
                name: "CTRL".into(),
                offset: 0,
                size: 32,
                access: "rw",
                fields: vec![FieldSchema {
                    name: "ENABLE".into(),
                    bits: [0, 0],
                }],
            }]
        }

        /// Answers the probe, and its answer happens to be zero.
        #[derive(Debug)]
        struct Answers;
        impl Peripheral for Answers {
            fn read(&self, _o: u64) -> SimResult<u8> {
                Ok(0)
            }
            fn write(&mut self, _o: u64, _v: u8) -> SimResult<()> {
                Ok(())
            }
            fn peek(&self, _o: u64) -> Option<u8> {
                Some(0)
            }
            fn describe_registers(&self) -> Option<Vec<RegisterSchema>> {
                Some(schema())
            }
        }

        /// Models nothing probe-able — the overwhelmingly common case.
        #[derive(Debug)]
        struct Silent;
        impl Peripheral for Silent {
            fn read(&self, _o: u64) -> SimResult<u8> {
                Ok(0)
            }
            fn write(&mut self, _o: u64, _v: u8) -> SimResult<()> {
                Ok(())
            }
            fn describe_registers(&self) -> Option<Vec<RegisterSchema>> {
                Some(schema())
            }
        }

        let opts = InspectOpts::default();
        let answered = default_inspect(&Answers, 0, "answers", &opts);
        let silent = default_inspect(&Silent, 0, "silent", &opts);

        assert_eq!(
            answered.registers[0].value,
            Some(0),
            "a model that really reports zero still reports zero"
        );
        assert_eq!(
            answered.registers[0].fields.len(),
            1,
            "fields decode from a real word"
        );

        assert_eq!(
            silent.registers[0].value, None,
            "no answer must not be reported as the number zero"
        );
        assert!(
            silent.registers[0].fields.is_empty(),
            "a field sliced out of a word nobody supplied is the same fabrication"
        );
        assert_eq!(
            silent.registers[0].name, "CTRL",
            "the register is still NAMED — the schema is honest about layout, \
             it is the value that was never real"
        );

        // And the difference survives serialization, which is what every
        // out-of-process consumer actually sees.
        let a = serde_json::to_value(&answered.registers[0]).expect("serialize");
        let s = serde_json::to_value(&silent.registers[0]).expect("serialize");
        assert_eq!(a["value"], serde_json::json!(0));
        assert_eq!(s["value"], serde_json::Value::Null);
    }

    /// The same rule applies to the externally-supplied-schema path, which is
    /// where the great majority of named native registers come from (the SVD
    /// import). It must not be a second, laxer implementation.
    #[test]
    fn externally_named_registers_are_also_null_when_unanswered() {
        use crate::{Peripheral, SimResult};

        #[derive(Debug)]
        struct Silent;
        impl Peripheral for Silent {
            fn read(&self, _o: u64) -> SimResult<u8> {
                Ok(0)
            }
            fn write(&mut self, _o: u64, _v: u8) -> SimResult<()> {
                Ok(())
            }
        }

        let schema = vec![RegisterSchema {
            name: "SR".into(),
            offset: 4,
            size: 32,
            access: "ro",
            fields: vec![FieldSchema {
                name: "BUSY".into(),
                bits: [7, 7],
            }],
        }];
        let pi = inspect_with_schema(&Silent, 0x4000_0000, "quiet", &schema);
        assert_eq!(pi.registers[0].value, None);
        assert!(pi.registers[0].fields.is_empty());
    }

    /// A partly-answered word is reported unreadable rather than half-invented.
    #[test]
    fn partially_answered_word_is_unreadable() {
        use crate::{Peripheral, SimResult};

        #[derive(Debug)]
        struct HalfSpoken;
        impl Peripheral for HalfSpoken {
            fn read(&self, _o: u64) -> SimResult<u8> {
                Ok(0)
            }
            fn write(&mut self, _o: u64, _v: u8) -> SimResult<()> {
                Ok(())
            }
            /// Only the low half of the 32-bit word is modeled.
            fn peek(&self, o: u64) -> Option<u8> {
                (o < 2).then_some(0xFF)
            }
        }

        assert_eq!(peek_word(&HalfSpoken, 0, 16), Some(0xFFFF), "fully modeled");
        assert_eq!(
            peek_word(&HalfSpoken, 0, 32),
            None,
            "half from the model and half invented is not a value"
        );
    }
}
