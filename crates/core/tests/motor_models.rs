// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.

use labwired_core::physics::motor::{
    BrushedDcMotor, BrushedMotorParams, HBridgeCommand, HBridgeState, MotorFaults, ShaftParams,
};

const DT_S: f64 = 0.00001;

fn params(load_torque_nm: f64) -> BrushedMotorParams {
    BrushedMotorParams {
        resistance_ohm: 2.0,
        inductance_h: 0.001,
        torque_constant_nm_per_a: 0.02,
        back_emf_constant_v_per_rad_s: 0.02,
        supply_voltage_v: 6.0,
        shaft: ShaftParams {
            inertia_kg_m2: 0.0001,
            viscous_friction_nm_per_rad_s: 0.00001,
            load_torque_nm,
        },
    }
}

fn step_many(motor: &mut BrushedDcMotor, command: HBridgeCommand, steps: usize) {
    for _ in 0..steps {
        motor.step(command, DT_S).unwrap();
    }
}

#[test]
fn forward_accelerates_positive_and_reverse_accelerates_negative() {
    let mut forward = BrushedDcMotor::new(params(0.0)).unwrap();
    let mut reverse = BrushedDcMotor::new(params(0.0)).unwrap();

    step_many(&mut forward, HBridgeCommand::forward(1.0).unwrap(), 1_000);
    step_many(&mut reverse, HBridgeCommand::reverse(1.0).unwrap(), 1_000);

    assert!(forward.snapshot().angular_velocity_rad_s > 0.0);
    assert!(reverse.snapshot().angular_velocity_rad_s < 0.0);
}

#[test]
fn back_emf_bounds_no_load_current_and_speed() {
    let mut motor = BrushedDcMotor::new(params(0.0)).unwrap();

    step_many(&mut motor, HBridgeCommand::forward(1.0).unwrap(), 100_000);

    let snapshot = motor.snapshot();
    assert!(snapshot.angular_velocity_rad_s > 200.0);
    assert!(snapshot.angular_velocity_rad_s < 320.0);
    assert!(snapshot.current_a > 0.0);
    assert!(snapshot.current_a < 0.5);
}

#[test]
fn increased_load_reduces_steady_speed_and_increases_current() {
    let mut no_load = BrushedDcMotor::new(params(0.0)).unwrap();
    let mut loaded = BrushedDcMotor::new(params(0.01)).unwrap();
    let command = HBridgeCommand::forward(1.0).unwrap();

    step_many(&mut no_load, command, 100_000);
    step_many(&mut loaded, command, 100_000);

    let no_load = no_load.snapshot();
    let loaded = loaded.snapshot();
    assert!(loaded.angular_velocity_rad_s < no_load.angular_velocity_rad_s);
    assert!(loaded.current_a > no_load.current_a);
}

#[test]
fn brake_opposes_rotation_while_coast_only_decays_current() {
    let mut braking = BrushedDcMotor::new(params(0.0)).unwrap();
    step_many(&mut braking, HBridgeCommand::forward(1.0).unwrap(), 20_000);
    let driven = braking.snapshot();

    step_many(&mut braking, HBridgeCommand::brake(), 2_000);
    let braked = braking.snapshot();
    assert!(braked.electromagnetic_torque_nm < 0.0);
    assert!(braked.angular_velocity_rad_s < driven.angular_velocity_rad_s);

    let mut coasting = BrushedDcMotor::new(params(0.0)).unwrap();
    coasting
        .step(HBridgeCommand::forward(1.0).unwrap(), DT_S)
        .unwrap();
    let driven_current = coasting.snapshot().current_a;
    step_many(&mut coasting, HBridgeCommand::coast(), 1_000);
    assert!(coasting.snapshot().current_a.abs() < driven_current.abs());

    let mut at_rest = BrushedDcMotor::new(params(0.0)).unwrap();
    step_many(&mut at_rest, HBridgeCommand::coast(), 1_000);
    assert_eq!(at_rest.snapshot().angular_velocity_rad_s, 0.0);
    assert_eq!(at_rest.snapshot().current_a, 0.0);
}

#[test]
fn stall_allows_current_but_holds_position_and_release_starts_from_zero() {
    let mut motor = BrushedDcMotor::new(params(0.0)).unwrap();
    let command = HBridgeCommand::forward(1.0).unwrap();
    step_many(&mut motor, command, 2_000);
    let moving = motor.snapshot();
    assert!(moving.angular_velocity_rad_s > 0.0);

    motor.set_faults(MotorFaults { stalled: true });
    let engaged = motor.snapshot();
    assert_eq!(engaged.position_rad, moving.position_rad);
    assert_eq!(engaged.angular_velocity_rad_s, 0.0);

    step_many(&mut motor, command, 1_000);
    let stalled = motor.snapshot();
    assert_eq!(stalled.position_rad, engaged.position_rad);
    assert_eq!(stalled.angular_velocity_rad_s, 0.0);
    assert!(stalled.current_a > engaged.current_a);

    motor.set_faults(MotorFaults { stalled: false });
    assert_eq!(motor.snapshot().angular_velocity_rad_s, 0.0);
    motor.step(command, DT_S).unwrap();
    assert!(motor.snapshot().angular_velocity_rad_s > 0.0);
}

#[test]
fn rejected_steps_leave_the_entire_snapshot_unchanged() {
    let mut motor = BrushedDcMotor::new(params(0.0)).unwrap();
    motor
        .step(HBridgeCommand::forward(0.5).unwrap(), DT_S)
        .unwrap();
    let before = motor.snapshot();

    let invalid_command = HBridgeCommand {
        state: HBridgeState::Forward,
        duty: f64::NAN,
    };
    assert_eq!(motor.step(invalid_command, DT_S).unwrap_err().field, "duty");
    assert_eq!(motor.snapshot(), before);

    let mut overflowing_current = params(0.0);
    overflowing_current.resistance_ohm = f64::MIN_POSITIVE;
    overflowing_current.inductance_h = f64::MIN_POSITIVE;
    overflowing_current.supply_voltage_v = f64::MAX;
    let mut overflowing_current = BrushedDcMotor::new(overflowing_current).unwrap();
    let before_overflow = overflowing_current.snapshot();
    assert_eq!(
        overflowing_current
            .step(HBridgeCommand::forward(1.0).unwrap(), 1.0)
            .unwrap_err()
            .field,
        "current_a"
    );
    assert_eq!(overflowing_current.snapshot(), before_overflow);

    let mut tiny_inertia = params(0.0);
    tiny_inertia.resistance_ohm = 1.0;
    tiny_inertia.inductance_h = 100.0;
    tiny_inertia.torque_constant_nm_per_a = 1.0;
    tiny_inertia.back_emf_constant_v_per_rad_s = 1.0;
    tiny_inertia.supply_voltage_v = 1.0;
    tiny_inertia.shaft.inertia_kg_m2 = f64::MIN_POSITIVE;
    tiny_inertia.shaft.viscous_friction_nm_per_rad_s = 0.0;
    let mut motor = BrushedDcMotor::new(tiny_inertia).unwrap();
    let before = motor.snapshot();

    assert_eq!(
        motor
            .step(HBridgeCommand::forward(1.0).unwrap(), 10.0)
            .unwrap_err()
            .field,
        "state"
    );
    assert_eq!(motor.snapshot(), before);
}

#[test]
fn electrical_timestep_outside_stability_envelope_is_rejected_atomically() {
    let mut motor = BrushedDcMotor::new(params(0.0)).unwrap();
    let before = motor.snapshot();
    let electrical_limit_s = 2.0 * motor.params().inductance_h / motor.params().resistance_ohm;
    let outside_limit_s = f64::from_bits(electrical_limit_s.to_bits() + 1);

    let error = motor
        .step(HBridgeCommand::forward(1.0).unwrap(), outside_limit_s)
        .unwrap_err();

    assert_eq!(error.field, "dt_s");
    assert!(error.message.contains("electrical"));
    assert_eq!(motor.snapshot(), before);
}

#[test]
fn mechanical_timestep_outside_stability_envelope_is_rejected_atomically() {
    let mut mechanical_limited = params(0.0);
    mechanical_limited.resistance_ohm = 1.0;
    mechanical_limited.inductance_h = 100.0;
    mechanical_limited.shaft.inertia_kg_m2 = 1.0;
    mechanical_limited.shaft.viscous_friction_nm_per_rad_s = 1.0;
    let mut motor = BrushedDcMotor::new(mechanical_limited).unwrap();
    let before = motor.snapshot();
    let mechanical_limit_s = 2.0 * motor.params().shaft.inertia_kg_m2
        / motor.params().shaft.viscous_friction_nm_per_rad_s;
    let outside_limit_s = f64::from_bits(mechanical_limit_s.to_bits() + 1);

    let error = motor
        .step(HBridgeCommand::forward(1.0).unwrap(), outside_limit_s)
        .unwrap_err();

    assert_eq!(error.field, "dt_s");
    assert!(error.message.contains("mechanical"));
    assert_eq!(motor.snapshot(), before);
}

#[test]
fn identical_command_streams_are_exactly_deterministic() {
    let mut first = BrushedDcMotor::new(params(0.001)).unwrap();
    let mut second = BrushedDcMotor::new(params(0.001)).unwrap();

    for step in 0..10_000 {
        let command = match step % 4 {
            0 => HBridgeCommand::forward(0.75).unwrap(),
            1 => HBridgeCommand::reverse(0.25).unwrap(),
            2 => HBridgeCommand::brake(),
            _ => HBridgeCommand::coast(),
        };
        first.step(command, DT_S).unwrap();
        second.step(command, DT_S).unwrap();
    }

    assert_eq!(first.params(), second.params());
    assert_eq!(first.snapshot(), second.snapshot());
}
