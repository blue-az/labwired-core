// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.

use std::f64::consts::TAU;

use super::{ModelError, Shaft, ShaftParams};

/// Electrical and mechanical parameters for a reduced-order brushed DC motor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushedMotorParams {
    pub resistance_ohm: f64,
    pub inductance_h: f64,
    pub torque_constant_nm_per_a: f64,
    pub back_emf_constant_v_per_rad_s: f64,
    pub supply_voltage_v: f64,
    pub shaft: ShaftParams,
}

/// Fault inputs applied to the motor model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MotorFaults {
    pub stalled: bool,
}

/// Observable state from a completed brushed-motor integration step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushedMotorSnapshot {
    pub current_a: f64,
    pub applied_voltage_v: f64,
    pub back_emf_v: f64,
    pub electromagnetic_torque_nm: f64,
    pub position_rad: f64,
    pub wrapped_position_rad: f64,
    pub angular_velocity_rad_s: f64,
    pub speed_rpm: f64,
    pub bridge_state: HBridgeState,
    pub faults: MotorFaults,
}

/// Electrical state selected by an H-bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HBridgeState {
    Forward,
    Reverse,
    Brake,
    Coast,
}

impl HBridgeState {
    /// Converts digital bridge inputs into a deterministic state.
    ///
    /// `enable = false` always selects [`Self::Coast`]. With the bridge
    /// enabled, `brake = true` always selects [`Self::Brake`]. Otherwise the
    /// input truth table is:
    ///
    /// | `in1` | `in2` | State |
    /// |---|---|---|
    /// | false | false | Coast |
    /// | true | false | Forward |
    /// | false | true | Reverse |
    /// | true | true | Brake |
    pub fn from_pins(enable: bool, in1: bool, in2: bool, brake: bool) -> Self {
        if !enable {
            return Self::Coast;
        }
        if brake {
            return Self::Brake;
        }
        match (in1, in2) {
            (false, false) => Self::Coast,
            (true, false) => Self::Forward,
            (false, true) => Self::Reverse,
            (true, true) => Self::Brake,
        }
    }
}

/// H-bridge state and normalized PWM duty for one fixed motor step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HBridgeCommand {
    pub state: HBridgeState,
    pub duty: f64,
}

impl HBridgeCommand {
    pub fn forward(duty: f64) -> Result<Self, ModelError> {
        Self::driven(HBridgeState::Forward, duty)
    }

    pub fn reverse(duty: f64) -> Result<Self, ModelError> {
        Self::driven(HBridgeState::Reverse, duty)
    }

    pub fn brake() -> Self {
        Self {
            state: HBridgeState::Brake,
            duty: 0.0,
        }
    }

    pub fn coast() -> Self {
        Self {
            state: HBridgeState::Coast,
            duty: 0.0,
        }
    }

    fn driven(state: HBridgeState, duty: f64) -> Result<Self, ModelError> {
        validate_duty(duty)?;
        Ok(Self { state, duty })
    }

    fn validate(self) -> Result<(), ModelError> {
        validate_duty(self.duty)
    }
}

/// Deterministic fixed-step reduced-order brushed DC motor.
#[derive(Debug, Clone)]
pub struct BrushedDcMotor {
    params: BrushedMotorParams,
    shaft: Shaft,
    current_a: f64,
    applied_voltage_v: f64,
    back_emf_v: f64,
    electromagnetic_torque_nm: f64,
    bridge_state: HBridgeState,
    faults: MotorFaults,
}

impl BrushedDcMotor {
    pub fn new(params: BrushedMotorParams) -> Result<Self, ModelError> {
        validate_positive("resistance_ohm", params.resistance_ohm)?;
        validate_positive("inductance_h", params.inductance_h)?;
        validate_positive("torque_constant_nm_per_a", params.torque_constant_nm_per_a)?;
        validate_positive(
            "back_emf_constant_v_per_rad_s",
            params.back_emf_constant_v_per_rad_s,
        )?;
        validate_positive("supply_voltage_v", params.supply_voltage_v)?;
        let shaft = Shaft::new(params.shaft)?;

        Ok(Self {
            params,
            shaft,
            current_a: 0.0,
            applied_voltage_v: 0.0,
            back_emf_v: 0.0,
            electromagnetic_torque_nm: 0.0,
            bridge_state: HBridgeState::Coast,
            faults: MotorFaults::default(),
        })
    }

    pub fn params(&self) -> BrushedMotorParams {
        self.params
    }

    pub fn faults(&self) -> MotorFaults {
        self.faults
    }

    /// Replaces active faults.
    ///
    /// Engaging stall immediately clears shaft velocity while retaining the
    /// current position. Releasing stall therefore resumes from rest.
    pub fn set_faults(&mut self, faults: MotorFaults) {
        if faults.stalled {
            self.shaft.hold_still();
        }
        self.faults = faults;
    }

    pub fn snapshot(&self) -> BrushedMotorSnapshot {
        let shaft = self.shaft.snapshot();
        BrushedMotorSnapshot {
            current_a: self.current_a,
            applied_voltage_v: self.applied_voltage_v,
            back_emf_v: self.back_emf_v,
            electromagnetic_torque_nm: self.electromagnetic_torque_nm,
            position_rad: shaft.position_rad,
            wrapped_position_rad: shaft.wrapped_position_rad,
            angular_velocity_rad_s: shaft.angular_velocity_rad_s,
            speed_rpm: shaft.angular_velocity_rad_s * 60.0 / TAU,
            bridge_state: self.bridge_state,
            faults: self.faults,
        }
    }

    /// Advances electrical and mechanical state by one fixed explicit step.
    ///
    /// Driven and brake winding integration uses explicit Euler. The step is
    /// rejected unless `dt_s * resistance_ohm / inductance_h < 2` and, when
    /// viscous friction is nonzero,
    /// `dt_s * viscous_friction_nm_per_rad_s / inertia_kg_m2 < 2`.
    pub fn step(&mut self, command: HBridgeCommand, dt_s: f64) -> Result<(), ModelError> {
        command.validate()?;
        validate_positive("dt_s", dt_s)?;
        validate_timestep(self.params, dt_s)?;

        let mut candidate = self.clone();
        if candidate.faults.stalled {
            candidate.shaft.hold_still();
        }

        candidate.applied_voltage_v = match command.state {
            HBridgeState::Forward => candidate.params.supply_voltage_v * command.duty,
            HBridgeState::Reverse => -candidate.params.supply_voltage_v * command.duty,
            HBridgeState::Brake | HBridgeState::Coast => 0.0,
        };
        validate_candidate("applied_voltage_v", candidate.applied_voltage_v)?;
        candidate.back_emf_v = candidate.params.back_emf_constant_v_per_rad_s
            * candidate.shaft.angular_velocity_rad_s();
        validate_candidate("back_emf_v", candidate.back_emf_v)?;

        let current_a = if command.state == HBridgeState::Coast {
            // An open bridge cannot sustain or generate winding current in this
            // reduced-order model. Decay uses the winding L/R time constant and
            // clamps at zero to stay sign-preserving for large fixed steps. It
            // deliberately does not model switching paths or switching losses.
            let decay = (1.0
                - dt_s * candidate.params.resistance_ohm / candidate.params.inductance_h)
                .clamp(0.0, 1.0);
            candidate.current_a * decay
        } else {
            let current_derivative_a_per_s = (candidate.applied_voltage_v
                - candidate.params.resistance_ohm * candidate.current_a
                - candidate.back_emf_v)
                / candidate.params.inductance_h;
            candidate.current_a + current_derivative_a_per_s * dt_s
        };
        validate_candidate("current_a", current_a)?;
        candidate.current_a = current_a;
        candidate.electromagnetic_torque_nm =
            candidate.params.torque_constant_nm_per_a * candidate.current_a;
        validate_candidate(
            "electromagnetic_torque_nm",
            candidate.electromagnetic_torque_nm,
        )?;
        if !candidate.faults.stalled {
            candidate
                .shaft
                .step(candidate.electromagnetic_torque_nm, dt_s)?;
        }
        candidate.bridge_state = command.state;
        *self = candidate;
        Ok(())
    }
}

fn validate_duty(duty: f64) -> Result<(), ModelError> {
    if !duty.is_finite() {
        return Err(ModelError {
            field: "duty",
            message: "must be finite".to_owned(),
        });
    }
    if !(0.0..=1.0).contains(&duty) {
        return Err(ModelError {
            field: "duty",
            message: "must be between zero and one inclusive".to_owned(),
        });
    }
    Ok(())
}

fn validate_positive(field: &'static str, value: f64) -> Result<(), ModelError> {
    if !value.is_finite() {
        return Err(ModelError {
            field,
            message: "must be finite".to_owned(),
        });
    }
    if value <= 0.0 {
        return Err(ModelError {
            field,
            message: "must be greater than zero".to_owned(),
        });
    }
    Ok(())
}

fn validate_candidate(field: &'static str, value: f64) -> Result<(), ModelError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(ModelError {
            field,
            message: "step would produce a non-finite motor state".to_owned(),
        })
    }
}

fn validate_timestep(params: BrushedMotorParams, dt_s: f64) -> Result<(), ModelError> {
    let electrical_ratio = dt_s * params.resistance_ohm / params.inductance_h;
    if !electrical_ratio.is_finite() || electrical_ratio >= 2.0 {
        return Err(ModelError {
            field: "dt_s",
            message: "exceeds the electrical explicit-integration stability envelope \
                      (dt_s * resistance_ohm / inductance_h must be less than 2)"
                .to_owned(),
        });
    }

    let friction = params.shaft.viscous_friction_nm_per_rad_s;
    if friction > 0.0 {
        let mechanical_ratio = dt_s * friction / params.shaft.inertia_kg_m2;
        if !mechanical_ratio.is_finite() || mechanical_ratio >= 2.0 {
            return Err(ModelError {
                field: "dt_s",
                message: "exceeds the mechanical explicit-integration stability envelope \
                          (dt_s * viscous_friction_nm_per_rad_s / inertia_kg_m2 must be less than 2)"
                    .to_owned(),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::{
        BrushedDcMotor, BrushedMotorParams, HBridgeCommand, HBridgeState, MotorFaults, ShaftParams,
    };

    fn motor_params() -> BrushedMotorParams {
        BrushedMotorParams {
            resistance_ohm: 2.0,
            inductance_h: 0.001,
            torque_constant_nm_per_a: 0.02,
            back_emf_constant_v_per_rad_s: 0.02,
            supply_voltage_v: 6.0,
            shaft: ShaftParams {
                inertia_kg_m2: 0.0001,
                viscous_friction_nm_per_rad_s: 0.00001,
                load_torque_nm: 0.0,
            },
        }
    }

    #[test]
    fn h_bridge_commands_validate_duty_and_select_state() {
        assert_eq!(
            HBridgeCommand::forward(0.25).unwrap().state,
            HBridgeState::Forward
        );
        assert_eq!(
            HBridgeCommand::reverse(0.75).unwrap().state,
            HBridgeState::Reverse
        );
        assert_eq!(HBridgeCommand::brake().state, HBridgeState::Brake);
        assert_eq!(HBridgeCommand::coast().state, HBridgeState::Coast);

        for duty in [-0.01, 1.01, f64::NAN, f64::INFINITY] {
            let error = HBridgeCommand::forward(duty).unwrap_err();
            assert_eq!(error.field, "duty");
        }
    }

    #[test]
    fn h_bridge_pin_truth_table_is_pure_and_complete() {
        assert_eq!(
            HBridgeState::from_pins(false, false, false, false),
            HBridgeState::Coast
        );
        assert_eq!(
            HBridgeState::from_pins(true, true, false, false),
            HBridgeState::Forward
        );
        assert_eq!(
            HBridgeState::from_pins(true, false, true, false),
            HBridgeState::Reverse
        );
        assert_eq!(
            HBridgeState::from_pins(true, false, false, false),
            HBridgeState::Coast
        );
        assert_eq!(
            HBridgeState::from_pins(true, true, true, false),
            HBridgeState::Brake
        );
        assert_eq!(
            HBridgeState::from_pins(true, true, false, true),
            HBridgeState::Brake
        );
    }

    #[test]
    fn motor_parameters_reject_non_positive_and_non_finite_values() {
        let mut cases = Vec::new();
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut params = motor_params();
            params.resistance_ohm = invalid;
            cases.push((params, "resistance_ohm"));

            let mut params = motor_params();
            params.inductance_h = invalid;
            cases.push((params, "inductance_h"));

            let mut params = motor_params();
            params.torque_constant_nm_per_a = invalid;
            cases.push((params, "torque_constant_nm_per_a"));

            let mut params = motor_params();
            params.back_emf_constant_v_per_rad_s = invalid;
            cases.push((params, "back_emf_constant_v_per_rad_s"));

            let mut params = motor_params();
            params.supply_voltage_v = invalid;
            cases.push((params, "supply_voltage_v"));
        }

        for (params, expected_field) in cases {
            let error = BrushedDcMotor::new(params).unwrap_err();
            assert_eq!(error.field, expected_field);
        }

        let mut params = motor_params();
        params.shaft.inertia_kg_m2 = 0.0;
        assert_eq!(
            BrushedDcMotor::new(params).unwrap_err().field,
            "inertia_kg_m2"
        );

        let mut params = motor_params();
        params.shaft.viscous_friction_nm_per_rad_s = -0.1;
        assert_eq!(
            BrushedDcMotor::new(params).unwrap_err().field,
            "viscous_friction_nm_per_rad_s"
        );

        let mut params = motor_params();
        params.shaft.load_torque_nm = f64::NAN;
        assert_eq!(
            BrushedDcMotor::new(params).unwrap_err().field,
            "load_torque_nm"
        );
    }

    #[test]
    fn first_electrical_step_matches_winding_equation() {
        let mut motor = BrushedDcMotor::new(motor_params()).unwrap();

        motor
            .step(HBridgeCommand::forward(1.0).unwrap(), 0.00001)
            .unwrap();

        let snapshot = motor.snapshot();
        assert!((snapshot.current_a - 0.06).abs() < 1e-12);
        assert_eq!(snapshot.applied_voltage_v, 6.0);
        assert_eq!(snapshot.back_emf_v, 0.0);
        assert!((snapshot.electromagnetic_torque_nm - 0.0012).abs() < 1e-12);
        assert_eq!(
            snapshot.speed_rpm,
            snapshot.angular_velocity_rad_s * 60.0 / TAU
        );
        assert_eq!(snapshot.bridge_state, HBridgeState::Forward);
        assert_eq!(snapshot.faults, MotorFaults::default());
    }
}
