use super::*;
use crate::physics::motor::{
    BldcMotor, BldcMotorParams, BrushedDcMotor, BrushedMotorParams, GatePair, HBridgeCommand,
    HBridgeState, InverterCommand, QuadratureEncoder, ShaftParams,
};
use labwired_config::{BldcMotorConfig, BrushedMotorConfig, MotorModelConfig, SystemManifest};

/// Motor physics timebase until chip descriptors expose one authoritative CPU
/// frequency. Simulator cycle deltas are deterministic; this conversion never
/// observes host time. Keep this named and isolated so a future descriptor
/// clock can replace it at construction.
const MOTOR_SIM_CYCLES_PER_SECOND: f64 = 1_000_000.0;

#[derive(Debug, Clone, Copy)]
pub(super) struct ResolvedPin {
    peripheral: usize,
    bit: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MotorSnapshot {
    pub id: String,
    pub kind: &'static str,
    pub position_rad: f64,
    pub speed_rpm: f64,
    pub torque_nm: f64,
    pub current_a: Option<f64>,
    pub phase_currents_a: Option<[f64; 3]>,
    pub bus_voltage_v: f64,
    pub commutation_sector: Option<u8>,
    pub control_state: String,
    pub faults: Vec<String>,
}

pub(super) enum MotorRuntime {
    Dc {
        id: String,
        plant: BrushedDcMotor,
        encoder: QuadratureEncoder,
        pwm: ResolvedPin,
        direction: ResolvedPin,
        brake: ResolvedPin,
        enable: ResolvedPin,
        feedback: [ResolvedPin; 2],
        index: Option<ResolvedPin>,
    },
    Bldc {
        id: String,
        plant: BldcMotor,
        encoder: QuadratureEncoder,
        timer: usize,
        enable: ResolvedPin,
        hall: [ResolvedPin; 3],
        feedback: [ResolvedPin; 2],
        index: Option<ResolvedPin>,
    },
}

impl SystemBus {
    pub(super) fn install_motor_models(&mut self, manifest: &SystemManifest) -> anyhow::Result<()> {
        for config in manifest.resolved_motor_models()? {
            self.motors.push(match config {
                MotorModelConfig::Dc(config) => self.build_dc_motor(config)?,
                MotorModelConfig::Bldc(config) => self.build_bldc_motor(config)?,
            });
        }
        self.motor_cycle_anchor = self.current_cycle;
        Ok(())
    }

    fn resolve_motor_pin(
        &self,
        motor: &str,
        role: &str,
        label: &str,
    ) -> anyhow::Result<ResolvedPin> {
        let (addr, bit) = Self::resolve_pin_odr(self, label).ok_or_else(|| {
            anyhow::anyhow!("motor '{motor}': {role} pin '{label}' is not a compatible GPIO pin")
        })?;
        let peripheral = self.find_peripheral_index(addr).ok_or_else(|| {
            anyhow::anyhow!("motor '{motor}': {role} pin '{label}' has no GPIO peripheral")
        })?;
        Ok(ResolvedPin { peripheral, bit })
    }

    fn resolve_motor_input(
        &self,
        motor: &str,
        role: &str,
        label: &str,
    ) -> anyhow::Result<ResolvedPin> {
        let (addr, bit) = Self::resolve_pin_idr(self, label).ok_or_else(|| {
            anyhow::anyhow!("motor '{motor}': {role} pin '{label}' is not a compatible GPIO input")
        })?;
        let peripheral = self.find_peripheral_index(addr).ok_or_else(|| {
            anyhow::anyhow!("motor '{motor}': {role} pin '{label}' has no GPIO peripheral")
        })?;
        Ok(ResolvedPin { peripheral, bit })
    }

    fn build_dc_motor(&self, c: BrushedMotorConfig) -> anyhow::Result<MotorRuntime> {
        let shaft = ShaftParams {
            inertia_kg_m2: c.rotor_inertia_kg_m2,
            viscous_friction_nm_per_rad_s: c.viscous_friction_nm_per_rad_s,
            load_torque_nm: c.load_torque_nm,
        };
        let plant = BrushedDcMotor::new(BrushedMotorParams {
            resistance_ohm: c.resistance_ohm,
            inductance_h: c.inductance_h,
            torque_constant_nm_per_a: c.torque_constant_nm_per_a,
            back_emf_constant_v_per_rad_s: c.back_emf_constant_v_per_rad_s,
            supply_voltage_v: c.supply_voltage_v,
            shaft,
        })?;
        Ok(MotorRuntime::Dc {
            pwm: self.resolve_motor_pin(&c.id, "pwm", &c.pwm_pin)?,
            direction: self.resolve_motor_pin(&c.id, "direction", &c.direction_pin)?,
            brake: self.resolve_motor_pin(&c.id, "brake", &c.brake_pin)?,
            enable: self.resolve_motor_pin(&c.id, "enable", &c.enable_pin)?,
            feedback: [
                self.resolve_motor_input(&c.id, "encoder A", &c.encoder_a_pin)?,
                self.resolve_motor_input(&c.id, "encoder B", &c.encoder_b_pin)?,
            ],
            index: c
                .encoder_index_pin
                .as_deref()
                .map(|p| self.resolve_motor_input(&c.id, "encoder index", p))
                .transpose()?,
            encoder: QuadratureEncoder::new(c.encoder_cpr)?,
            id: c.id,
            plant,
        })
    }

    fn build_bldc_motor(&self, c: BldcMotorConfig) -> anyhow::Result<MotorRuntime> {
        // Resolve all declared phase pads now, even though TIM1 owns their
        // runtime levels. This rejects nonexistent/incompatible AF bindings
        // before firmware starts.
        for (role, pin) in [
            ("phase A high", &c.phase_a_high_pin),
            ("phase A low", &c.phase_a_low_pin),
            ("phase B high", &c.phase_b_high_pin),
            ("phase B low", &c.phase_b_low_pin),
            ("phase C high", &c.phase_c_high_pin),
            ("phase C low", &c.phase_c_low_pin),
        ] {
            self.resolve_motor_pin(&c.id, role, pin)?;
        }
        let timer = self.find_peripheral_index_by_name("tim1").ok_or_else(|| {
            anyhow::anyhow!("motor '{}': BLDC requires advanced timer 'tim1'", c.id)
        })?;
        let is_timer = self.peripherals[timer]
            .dev
            .as_any()
            .and_then(|a| a.downcast_ref::<crate::peripherals::timer::Timer>())
            .is_some();
        if !is_timer {
            anyhow::bail!("motor '{}': peripheral 'tim1' is not an STM32 timer", c.id);
        }
        let plant = BldcMotor::new(BldcMotorParams {
            resistance_ohm: c.resistance_ohm,
            inductance_h: c.inductance_h,
            torque_constant_nm_per_a: c.torque_constant_nm_per_a,
            back_emf_constant_v_per_rad_s: c.back_emf_constant_v_per_rad_s,
            supply_voltage_v: c.supply_voltage_v,
            pole_pairs: c.pole_pairs,
            shaft: ShaftParams {
                inertia_kg_m2: c.rotor_inertia_kg_m2,
                viscous_friction_nm_per_rad_s: c.viscous_friction_nm_per_rad_s,
                load_torque_nm: c.load_torque_nm,
            },
        })?;
        Ok(MotorRuntime::Bldc {
            id: c.id.clone(),
            plant,
            encoder: QuadratureEncoder::new(c.encoder_cpr)?,
            timer,
            enable: self.resolve_motor_pin(&c.id, "enable", &c.enable_pin)?,
            hall: [
                self.resolve_motor_input(&c.id, "Hall A", &c.hall_a_pin)?,
                self.resolve_motor_input(&c.id, "Hall B", &c.hall_b_pin)?,
                self.resolve_motor_input(&c.id, "Hall C", &c.hall_c_pin)?,
            ],
            feedback: [
                self.resolve_motor_input(&c.id, "encoder A", &c.encoder_a_pin)?,
                self.resolve_motor_input(&c.id, "encoder B", &c.encoder_b_pin)?,
            ],
            index: c
                .encoder_index_pin
                .as_deref()
                .map(|p| self.resolve_motor_input(&c.id, "encoder index", p))
                .transpose()?,
        })
    }

    fn pin_output(&self, pin: ResolvedPin) -> bool {
        self.peripherals[pin.peripheral]
            .dev
            .read_gpio_output(pin.bit)
            .unwrap_or(false)
    }

    fn drive_input(&mut self, pin: ResolvedPin, level: bool) {
        let _ = self.peripherals[pin.peripheral]
            .dev
            .set_gpio_input(pin.bit, level);
    }

    pub(super) fn service_motor_models(&mut self) {
        let elapsed = self.current_cycle.saturating_sub(self.motor_cycle_anchor);
        if elapsed == 0 || self.motors.is_empty() {
            return;
        }
        self.motor_cycle_anchor = self.current_cycle;
        let dt_s = elapsed as f64 / MOTOR_SIM_CYCLES_PER_SECOND;
        let mut motors = std::mem::take(&mut self.motors);
        for motor in &mut motors {
            match motor {
                MotorRuntime::Dc {
                    plant,
                    encoder,
                    pwm,
                    direction,
                    brake,
                    enable,
                    feedback,
                    index,
                    ..
                } => {
                    let enabled = self.pin_output(*enable);
                    let braking = self.pin_output(*brake);
                    let duty = f64::from(self.pin_output(*pwm));
                    let state = if !enabled {
                        HBridgeState::Coast
                    } else if braking {
                        HBridgeState::Brake
                    } else if self.pin_output(*direction) {
                        HBridgeState::Forward
                    } else {
                        HBridgeState::Reverse
                    };
                    let command = match state {
                        HBridgeState::Forward => HBridgeCommand::forward(duty),
                        HBridgeState::Reverse => HBridgeCommand::reverse(duty),
                        HBridgeState::Brake => Ok(HBridgeCommand::brake()),
                        HBridgeState::Coast => Ok(HBridgeCommand::coast()),
                    };
                    if let Ok(command) = command {
                        let params = plant.params();
                        for step_s in stable_substeps(
                            dt_s,
                            0.25 * params.inductance_h / params.resistance_ohm,
                        ) {
                            if plant.step(command, step_s).is_err() {
                                break;
                            }
                        }
                    }
                    let pins = encoder.sample(plant.snapshot().position_rad).ok();
                    if let Some(pins) = pins {
                        self.drive_input(feedback[0], pins.a);
                        self.drive_input(feedback[1], pins.b);
                        if let Some(index) = index {
                            self.drive_input(*index, pins.index);
                        }
                    }
                }
                MotorRuntime::Bldc {
                    plant,
                    encoder,
                    timer,
                    enable,
                    hall,
                    feedback,
                    index,
                    ..
                } => {
                    let timer_output = self.peripherals[*timer]
                        .dev
                        .as_any()
                        .and_then(|a| a.downcast_ref::<crate::peripherals::timer::Timer>())
                        .map(crate::peripherals::timer::Timer::output_snapshot);
                    let command = timer_output
                        .filter(|pwm| pwm.main_output_enabled && self.pin_output(*enable))
                        .map(|pwm| InverterCommand {
                            enabled: true,
                            phase_a: sampled_gate_pair(pwm.channels[0]),
                            phase_b: sampled_gate_pair(pwm.channels[1]),
                            phase_c: sampled_gate_pair(pwm.channels[2]),
                        })
                        .unwrap_or_else(InverterCommand::off);
                    let params = plant.params();
                    for step_s in
                        stable_substeps(dt_s, 0.25 * params.inductance_h / params.resistance_ohm)
                    {
                        if plant.step(command, step_s).is_err() {
                            break;
                        }
                    }
                    let snapshot = plant.snapshot();
                    for (bit, pin) in hall.iter().enumerate() {
                        self.drive_input(*pin, snapshot.hall_state & (1 << bit) != 0);
                    }
                    if let Ok(pins) = encoder.sample(snapshot.position_rad) {
                        self.drive_input(feedback[0], pins.a);
                        self.drive_input(feedback[1], pins.b);
                        if let Some(index) = index {
                            self.drive_input(*index, pins.index);
                        }
                    }
                }
            }
        }
        self.motors = motors;
    }

    pub fn motor_snapshots(&self) -> Vec<MotorSnapshot> {
        self.motors
            .iter()
            .map(|motor| match motor {
                MotorRuntime::Dc { id, plant, .. } => {
                    let s = plant.snapshot();
                    MotorSnapshot {
                        id: id.clone(),
                        kind: "dc",
                        position_rad: s.position_rad,
                        speed_rpm: s.speed_rpm,
                        torque_nm: s.electromagnetic_torque_nm,
                        current_a: Some(s.current_a),
                        phase_currents_a: None,
                        bus_voltage_v: plant.params().supply_voltage_v,
                        commutation_sector: None,
                        control_state: format!("{:?}", s.bridge_state).to_ascii_lowercase(),
                        faults: s
                            .faults
                            .stalled
                            .then(|| "stalled".to_owned())
                            .into_iter()
                            .collect(),
                    }
                }
                MotorRuntime::Bldc { id, plant, .. } => {
                    let s = plant.snapshot();
                    let mut faults = Vec::new();
                    if s.faults.stalled {
                        faults.push("stalled".to_owned());
                    }
                    if s.faults.open_phase.is_some() {
                        faults.push("open-phase".to_owned());
                    }
                    if !s.inverter_faults.is_empty() {
                        faults.push("inverter".to_owned());
                    }
                    MotorSnapshot {
                        id: id.clone(),
                        kind: "bldc",
                        position_rad: s.position_rad,
                        speed_rpm: s.speed_rpm,
                        torque_nm: s.electromagnetic_torque_nm,
                        current_a: Some(s.dc_bus_current_a),
                        phase_currents_a: Some(s.phase_currents_a),
                        bus_voltage_v: s.dc_bus_voltage_v,
                        commutation_sector: Some(s.commutation_sector),
                        control_state: if s.dc_bus_voltage_v > 0.0 {
                            "inverter".to_owned()
                        } else {
                            "off".to_owned()
                        },
                        faults,
                    }
                }
            })
            .collect()
    }
}

/// Cycle-average PWM is sampled at normalized phase 0.5. This is deterministic
/// and honors duty/polarity/complementary enable without pretending the reduced
/// motor model resolves switching edges. Dead-time remains observable in the
/// timer snapshot but cannot be represented by the plant's boolean gate API.
fn sampled_gate_pair(channel: crate::peripherals::timer::TimerChannelOutputSnapshot) -> GatePair {
    let main_level = channel.enabled && ((0.5 < channel.duty_fraction) ^ channel.active_low);
    let complementary_level = channel.complementary_enabled
        && ((0.5 >= channel.duty_fraction) ^ channel.complementary_active_low);
    GatePair {
        high: main_level,
        low: complementary_level,
    }
}

fn stable_substeps(total_s: f64, max_step_s: f64) -> impl Iterator<Item = f64> {
    let count = (total_s / max_step_s).ceil().max(1.0) as u64;
    std::iter::repeat_n(total_s / count as f64, count as usize)
}
