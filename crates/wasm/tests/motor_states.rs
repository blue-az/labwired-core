use labwired_config::{ChipDescriptor, SystemManifest};
use labwired_core::system::cortex_m::configure_cortex_m;
use labwired_core::{AdvanceRequest, Machine};

fn snapshot(machine: &Machine<labwired_core::cpu::cortex_m::CortexM>) -> serde_json::Value {
    let state = machine.bus.motor_snapshots().remove(0);
    serde_json::json!({
        "id": state.id,
        "kind": match state.kind {
            "dc" => "dc-motor",
            "bldc" => "bldc-motor",
            other => other,
        },
        "position_rad": state.position_rad,
        "speed_rpm": state.speed_rpm,
        "torque_nm": state.torque_nm,
        "current_a": state.current_a,
        "phase_currents_a": state.phase_currents_a,
        "bus_voltage_v": state.bus_voltage_v,
        "commutation_sector": state.commutation_sector,
        "control_state": state.control_state,
        "faults": state.faults,
    })
}

fn run_native_trace() -> Vec<serde_json::Value> {
    let chip: ChipDescriptor =
        serde_yaml::from_str(include_str!("../../../configs/chips/stm32l476.yaml")).unwrap();
    let manifest: SystemManifest = serde_yaml::from_str(include_str!(
        "../../../examples/nucleo-l476rg-bldc/system.yaml"
    ))
    .unwrap();
    let mut bus = labwired_core::bus::SystemBus::from_config(&chip, &manifest).unwrap();
    let (cpu, _) = configure_cortex_m(&mut bus);
    let mut machine = Machine::new(cpu, bus);
    let image =
        labwired_loader::load_elf_bytes(include_bytes!("fixtures/firmware-l476-bldc-six-step.elf"))
            .unwrap();
    machine.load_firmware(&image).unwrap();

    let mut samples = Vec::new();
    machine.advance(AdvanceRequest::run(Some(100_000))).unwrap();
    samples.push(snapshot(&machine));

    machine
        .bus
        .set_motor_named_fault("drive_motor", "open-phase-a", true)
        .unwrap();
    machine.advance(AdvanceRequest::run(Some(50_000))).unwrap();
    samples.push(snapshot(&machine));

    machine
        .bus
        .set_motor_named_fault("drive_motor", "open-phase-a", false)
        .unwrap();
    machine
        .bus
        .set_motor_named_fault("drive_motor", "hall-b-low", true)
        .unwrap();
    machine.advance(AdvanceRequest::run(Some(50_000))).unwrap();
    samples.push(snapshot(&machine));

    machine
        .bus
        .set_motor_named_fault("drive_motor", "hall-b-low", false)
        .unwrap();
    machine
        .bus
        .set_motor_named_fault("drive_motor", "inverter", true)
        .unwrap();
    machine.advance(AdvanceRequest::run(Some(50_000))).unwrap();
    samples.push(snapshot(&machine));

    machine
        .bus
        .set_motor_named_fault("drive_motor", "inverter", false)
        .unwrap();
    machine.advance(AdvanceRequest::run(Some(50_000))).unwrap();
    samples.push(snapshot(&machine));
    samples
}

#[test]
fn native_motor_states_trace_matches_checked_parity_fixture() {
    let expected: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/motor-parity-native.json")).unwrap();
    let actual = run_native_trace();
    assert_eq!(actual.len(), expected.len());
    for (sample_index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        for key in [
            "id",
            "kind",
            "commutation_sector",
            "control_state",
            "faults",
        ] {
            assert_eq!(
                actual[key], expected[key],
                "sample {sample_index} field {key}"
            );
        }
        for key in [
            "position_rad",
            "speed_rpm",
            "torque_nm",
            "current_a",
            "bus_voltage_v",
        ] {
            let actual = actual[key].as_f64().unwrap();
            let expected = expected[key].as_f64().unwrap();
            let tolerance = 1e-12 * expected.abs().max(1.0);
            assert!(
                (actual - expected).abs() <= tolerance,
                "sample {sample_index} field {key}: {actual} != {expected} ± {tolerance}"
            );
        }
        for (phase, (actual, expected)) in actual["phase_currents_a"]
            .as_array()
            .unwrap()
            .iter()
            .zip(expected["phase_currents_a"].as_array().unwrap())
            .enumerate()
        {
            let actual = actual.as_f64().unwrap();
            let expected = expected.as_f64().unwrap();
            let tolerance = 1e-12 * expected.abs().max(1.0);
            assert!(
                (actual - expected).abs() <= tolerance,
                "sample {sample_index} phase {phase}: {actual} != {expected} ± {tolerance}"
            );
        }
    }
    assert_eq!(actual[3]["control_state"], "fault:inverter");
    assert_eq!(actual[3]["faults"], serde_json::json!(["inverter"]));
    let phase_magnitude = |sample: &serde_json::Value| {
        sample["phase_currents_a"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_f64().unwrap().abs())
            .sum::<f64>()
    };
    assert!(
        phase_magnitude(&actual[3]) < phase_magnitude(&actual[2]),
        "injected inverter fault must disconnect the driven phases"
    );
    assert_eq!(actual[4]["control_state"], "inverter");
    assert_eq!(actual[4]["faults"], serde_json::json!([]));
}
