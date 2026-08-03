// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! ONE walk over every device an author placed on this machine, whatever
//! transport binds it, and the ONE join that gives each one its manifest
//! identity.
//!
//! # Why this is not per-transport
//!
//! A placed part becomes attached to the machine in one of two structurally
//! different ways, and for a while only the first was visible to `inspect`:
//!
//! 1. **Owned by a controller.** An I²C slave or an SPI panel lives inside the
//!    controller peripheral it answers to, so it is reachable only by asking
//!    that controller — [`crate::Peripheral::for_each_attached_device`]. A walk
//!    over `SystemBus::peripherals` alone cannot see it. That is the bug that
//!    made a customer rig report 52 chip-internal peripherals and not one of
//!    the six parts on the canvas.
//! 2. **Resident on the bus.** An HC-SR04 on TRIG/ECHO, a servo on a PWM pad, a
//!    WS2812 strip, a thermistor driving an ADC channel, a CAN tester node —
//!    none of these answers to an address, so none can live inside a
//!    controller. Each is held in a typed collection on [`SystemBus`] instead.
//!
//! The second kind was invisible to `inspect` for exactly the same reason the
//! first kind had been: the walk did not go where the models are. Fixing it per
//! family would have meant an eighth, ninth, tenth special case — each one a
//! fresh chance to add a device that simulates correctly and reports nothing.
//!
//! So there is one walk ([`SystemBus::for_each_attached_device`]) that covers
//! both kinds, one borrowed record shape
//! ([`crate::inspect::AttachedDeviceRef`]) that both kinds emit, and one join
//! ([`SystemBus::inspect_devices`]) that turns either into the same
//! [`crate::inspect::DeviceInspect`]. A transport is a field of the record, not
//! a branch in the pipeline.
//!
//! # What the two kinds know about themselves
//!
//! They differ in exactly one respect, and the record shape carries it
//! explicitly rather than hiding it:
//!
//! * A controller-hosted device is identified by WHERE it answers. It states
//!   its address (or chip-select) and the controller states the bus; the
//!   manifest name is joined on afterwards by matching that placement.
//! * A bus-resident device has no address to be identified by, so it carries
//!   the `external_devices:` id it was stamped with when it was attached, and
//!   the join matches on that. This is not a fallback or a guess: the id is the
//!   author's own text, copied onto the model at attach.
//!
//! # The order is load-bearing
//!
//! Controller-hosted devices are walked first, bus-resident ones after. A
//! declaration can only be claimed once, so this guarantees the addressed join
//! — which has to do real disambiguation work — is never disturbed by a
//! name-matched device, and keeps the `devices` array a stable prefix of what
//! it was before bus-resident devices joined it.

use super::SystemBus;
use crate::inspect::{AttachedDeviceRef, DeviceEvidence};
// `component_id` is the SimInput identity stamp — the same one the stimulus
// walk resolves `set_input`'s `component:` against, so a device answers to the
// same name in `inspect` as it does in the stimulus API.
use crate::sim_input::SimInput;

impl SystemBus {
    /// Walk every attached (off-chip) device on this machine, calling
    /// `f(bus, device)` for each. `bus` is the peripheral the device hangs off
    /// — `Some("i2c0")`, `Some("gpioa")`, `Some("fdcan1")` — or `None` in the
    /// one case where the engine genuinely does not know (see
    /// [`crate::inspect::DeviceAttachment::bus`]).
    ///
    /// This is the ONE walk behind `inspect`'s `devices` array. A device
    /// reachable here is reachable from every inspect surface at once; a
    /// device NOT reachable here is invisible to all of them, which is why
    /// [`Self::for_each_bus_resident_device`] must list every collection of
    /// device models the bus holds. `attached_device_walk_covers_every_bus_
    /// collection` in `crates/core/tests/inspect_device_binding_universal.rs`
    /// is the gate that fails if a new one is added and not walked.
    ///
    /// Borrow discipline: everything happens inside the visitor.
    /// `AttachedDeviceRef` borrows the model, and a controller may be holding a
    /// `RefCell` borrow open for the duration of the call, so the reference
    /// deliberately cannot escape.
    pub fn for_each_attached_device(&self, f: &mut dyn FnMut(Option<&str>, AttachedDeviceRef<'_>)) {
        for entry in &self.peripherals {
            let name = entry.name.as_str();
            entry
                .dev
                .for_each_attached_device(&mut |d| f(Some(d.bus.unwrap_or(name)), d));
        }
        self.for_each_bus_resident_device(f);
    }

    /// The `external_devices:` connection recorded for `id`, when the manifest
    /// declared one. This is the author's own text, not an inference.
    fn declared_connection(&self, id: &str) -> Option<&str> {
        self.external_device_decls
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.connection.as_str())
    }

    /// Emit one bus-resident device.
    ///
    /// `r.bus` is what the MODEL knows (a CAN tester and an analog source both
    /// record their `connection:`); when it knows nothing, the declaration it
    /// was built from supplies it. Neither is a guess — both are the manifest
    /// text — and when there is no declaration either, the owner is reported as
    /// unknown rather than invented.
    fn emit_resident(
        &self,
        f: &mut dyn FnMut(Option<&str>, AttachedDeviceRef<'_>),
        r: Resident<'_>,
    ) {
        let bus = r.bus.or_else(|| self.declared_connection(r.declared_id));
        f(
            bus,
            AttachedDeviceRef {
                transport: r.transport,
                address: None,
                cs_pin: None,
                mux_address: None,
                channel: r.channel,
                declared_id: Some(r.declared_id),
                instance_id: r.instance_id,
                bus,
                evidence: r.evidence,
            },
        );
    }

    /// Every collection of author-placed device models that lives DIRECTLY on
    /// the bus rather than inside a controller.
    ///
    /// **Adding a collection to [`SystemBus`] means adding an arm here.** A
    /// model the bus holds but this function does not emit is a device that
    /// simulates and reports nothing — the exact failure this seam exists to
    /// make impossible. The gate test named on
    /// [`Self::for_each_attached_device`] enforces it at the source level, so a
    /// forgotten arm fails the build rather than one customer's rig.
    fn for_each_bus_resident_device(&self, f: &mut dyn FnMut(Option<&str>, AttachedDeviceRef<'_>)) {
        for dev in &self.hcsr04 {
            self.emit_resident(f, Resident::gpio(&dev.id, None));
        }
        for dev in &self.gpio_devices {
            self.emit_resident(f, Resident::gpio(dev.id(), None));
        }
        for dev in &self.ws2812 {
            let id = dev.component_id().unwrap_or("ws2812");
            self.emit_resident(f, Resident::gpio(id, None));
        }
        for dev in &self.servos {
            self.emit_resident(f, Resident::gpio(dev.id(), None));
        }
        for dev in &self.step_dir_motors {
            self.emit_resident(f, Resident::gpio(dev.id(), None));
        }
        for dev in &self.h_bridge_motors {
            // An H-bridge board carries two independent motor channels, so ONE
            // declaration builds TWO models (`<id>-a`, `<id>-b`). Each reports
            // its own channel identity and is joined to the declaration both
            // came from — neither is anonymous, and neither claims to be the
            // whole board.
            let r = match dev.declared_id() {
                Some(declared) => Resident::gpio(declared, None).instance(dev.id()),
                None => Resident::gpio(dev.id(), None),
            };
            self.emit_resident(f, r);
        }
        for dev in &self.unipolar_steppers {
            self.emit_resident(f, Resident::gpio(dev.id(), None));
        }
        for dev in &self.tm1637 {
            // A bus-resident DISPLAY: it reports evidence directly, because it
            // has no controller trait to hang it on.
            self.emit_resident(f, Resident::gpio(&dev.id, Some(dev)));
        }
        for dev in &self.hx711 {
            let id = dev.component_id().unwrap_or("hx711");
            self.emit_resident(f, Resident::gpio(id, None));
        }
        for dev in &self.seven_segment {
            self.emit_resident(f, Resident::gpio(&dev.id, Some(dev)));
        }
        for dev in &self.analog_inputs {
            // An analog source has no `Any` view, so it is listed but not read:
            // `model: None` says "cannot be read", never "has nothing to show".
            let id = dev.source.component_id().unwrap_or("analog");
            self.emit_resident(f, Resident::analog(id, &dev.connection, dev.channel));
        }
        for dev in &self.can_diagnostic_testers {
            self.emit_resident(f, Resident::can(&dev.id, &dev.connection));
        }
        for dev in &self.can_uds_testers {
            self.emit_resident(f, Resident::can(&dev.id, &dev.connection));
        }
        for dev in &self.can_log_players {
            self.emit_resident(f, Resident::can(&dev.id, &dev.connection));
        }
    }

    /// Enumerate the external (off-chip) devices attached to this machine,
    /// joining each live model to the manifest declaration that asked for it.
    ///
    /// Two halves, deliberately in that order:
    ///
    /// 1. **What is really there** comes from walking
    ///    ([`Self::for_each_attached_device`]). A device is listed because a
    ///    live model was found, never because a manifest mentioned one. A
    ///    declaration that failed to build (the `"unsupported type … skipping"`
    ///    path) therefore produces no entry — the record cannot claim a device
    ///    the engine does not have.
    /// 2. **What it is called** comes from
    ///    [`crate::bus::SystemBus::external_device_decls`]. When no declaration
    ///    matches, the id is synthesized from the attachment and `declared` is
    ///    `false`, so a caller can always tell a named device from a guessed
    ///    one.
    ///
    /// This lives on the bus because the bus is the one thing that holds BOTH
    /// the live models and the declarations. `Machine::inspect` delegates to it.
    pub fn inspect_devices(
        &self,
        filter: Option<&str>,
        opts: &crate::inspect::InspectOpts,
    ) -> Vec<crate::inspect::DeviceInspect> {
        let decls = &self.external_device_decls;
        // A declaration's controller is its own `connection` unless that names
        // another declaration (a bus switch), in which case it inherits the
        // switch's controller and address. Resolved once, up front.
        let effective: Vec<(usize, String, Option<u8>)> = decls
            .iter()
            .enumerate()
            .map(|(i, d)| match decls.iter().find(|p| p.id == d.connection) {
                Some(parent) => (i, parent.connection.clone(), parent.address),
                None => (i, d.connection.clone(), None),
            })
            .collect();

        let mut used = vec![false; decls.len()];
        let mut out = Vec::new();
        self.for_each_attached_device(&mut |bus, d| {
            if filter.is_some_and(|f| bus != Some(f)) {
                return;
            }
            // A device that states its own `external_devices:` id is joined by
            // that id — it has no address to be identified by, and the id is
            // the author's own text stamped onto the model at attach.
            let matched = match d.declared_id {
                // A declaration that fans out to several models (`instance_id`)
                // is claimed by all of them: `used` exists to stop two DIFFERENT
                // devices from taking one name, not to stop a board's own
                // channels from sharing theirs.
                Some(id) => decls
                    .iter()
                    .position(|decl| decl.id == id)
                    .filter(|&i| !used[i] || d.instance_id.is_some()),
                None => Self::match_addressed_decl(decls, &effective, &used, bus, &d),
            };
            if let Some(i) = matched {
                if d.instance_id.is_none() {
                    used[i] = true;
                }
            }

            let id = match (d.instance_id, matched, d.declared_id) {
                (Some(instance), _, _) => instance.to_string(),
                (None, Some(i), _) => decls[i].id.clone(),
                (None, None, Some(stamped)) => stamped.to_string(),
                (None, None, None) => {
                    let bus_name = bus.unwrap_or(d.transport);
                    match (d.address, d.cs_pin) {
                        (Some(a), _) => format!("{bus_name}@0x{a:02x}"),
                        (None, Some(cs)) => format!("{bus_name}@{cs}"),
                        (None, None) => bus_name.to_string(),
                    }
                }
            };
            // The device decides what it can show; this only supplies the id
            // it is addressed by, which is not known until the join above.
            let artifacts = d
                .evidence
                .map(|e| e.artifacts(&id, opts))
                .unwrap_or_default();
            out.push(crate::inspect::DeviceInspect {
                device_type: matched.map(|i| decls[i].device_type.clone()),
                declared: matched.is_some(),
                attachment: crate::inspect::DeviceAttachment {
                    transport: d.transport.to_string(),
                    bus: bus.map(str::to_string),
                    address: d.address,
                    cs_pin: d.cs_pin.map(str::to_string),
                    mux_address: d.mux_address,
                    channel: d.channel,
                },
                id,
                artifacts,
            });
        });
        out
    }

    /// The addressed join: which declaration, if any, describes the device
    /// found at this placement.
    ///
    /// A declaration that STATES an address (or chip-select) must match it.
    /// Only a declaration that states none falls back to "the one remaining
    /// candidate", which is the case where the manifest deliberately left the
    /// address to the model's own default. Without that split, the
    /// classic-ESP32 board BMP280 at 0x76 — real, on the bus, and declared by
    /// nobody — was handed the `mux` declaration's name purely for being found
    /// first, which is a fabricated identity.
    fn match_addressed_decl(
        decls: &[super::ExternalDeviceDecl],
        effective: &[(usize, String, Option<u8>)],
        used: &[bool],
        bus: Option<&str>,
        d: &AttachedDeviceRef<'_>,
    ) -> Option<usize> {
        let bus_name = bus?;
        // Candidates: declarations that sit on this controller, at this
        // bus-switch position, and are not already spoken for.
        let candidates: Vec<usize> = effective
            .iter()
            .filter(|(i, conn, mux_addr)| {
                !used[*i]
                    && conn == bus_name
                    && *mux_addr == d.mux_address
                    && decls[*i].channel == d.channel
            })
            .map(|(i, _, _)| *i)
            .collect();
        let key_of = |i: usize| match d.transport {
            "spi" => decls[i].cs_pin.is_some(),
            _ => decls[i].address.is_some(),
        };
        let hit = |i: usize| match d.transport {
            "spi" => decls[i].cs_pin.as_deref() == d.cs_pin,
            _ => decls[i].address == d.address,
        };
        let exact: Vec<usize> = candidates.iter().copied().filter(|&i| hit(i)).collect();
        if exact.len() == 1 {
            return Some(exact[0]);
        }
        if !exact.is_empty() {
            return None;
        }
        let loose: Vec<usize> = candidates.iter().copied().filter(|&i| !key_of(i)).collect();
        (loose.len() == 1).then(|| loose[0])
    }
}

/// One bus-resident device's placement, as the walk knows it — the argument
/// bundle of [`SystemBus::emit_resident`].
///
/// `transport` says how the device BINDS, never what it is: `"gpio"` covers
/// everything wired to pins (bit-banged protocols, one-wire framing,
/// quadrature, PWM), `"analog"` a source that drives an ADC channel's level,
/// `"can"` a second node on a CAN bus. The three constructors below are the
/// only ways to build one, so a new family has to say which of the three it is
/// rather than inventing a fourth vocabulary.
struct Resident<'a> {
    declared_id: &'a str,
    instance_id: Option<&'a str>,
    transport: &'static str,
    bus: Option<&'a str>,
    channel: Option<u8>,
    /// A bus-resident model that can show something of itself implements
    /// [`crate::inspect::DeviceEvidence`] directly and is passed here — the
    /// TM1637 and the direct-driven 7-segment digit are displays that happen to
    /// be wired to pins. `None` is the honest answer for everything else: a
    /// servo and a distance sensor have no display surface, and `None` says
    /// that rather than promising an empty screen.
    evidence: Option<&'a dyn DeviceEvidence>,
}

impl<'a> Resident<'a> {
    /// A device wired to MCU pins. Its owner comes from its declaration: the
    /// model holds pin addresses, not a peripheral name.
    fn gpio(declared_id: &'a str, evidence: Option<&'a dyn DeviceEvidence>) -> Self {
        Self {
            declared_id,
            instance_id: None,
            transport: "gpio",
            bus: None,
            channel: None,
            evidence,
        }
    }

    /// A source driving one ADC channel's level. The ADC channel is placement,
    /// exactly as a bus-switch channel is: "which channel of `bus`".
    fn analog(declared_id: &'a str, connection: &'a str, channel: u8) -> Self {
        Self {
            declared_id,
            instance_id: None,
            transport: "analog",
            bus: Some(connection),
            channel: Some(channel),
            evidence: None,
        }
    }

    /// A second node on a CAN bus, which records the controller it talks to.
    fn can(declared_id: &'a str, connection: &'a str) -> Self {
        Self {
            declared_id,
            instance_id: None,
            transport: "can",
            bus: Some(connection),
            channel: None,
            evidence: None,
        }
    }

    /// Mark this as ONE of several models built from a single declaration.
    fn instance(mut self, instance_id: &'a str) -> Self {
        self.instance_id = Some(instance_id);
        self
    }
}
