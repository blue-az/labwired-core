// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.

//! The run-manifest digest must be byte-identical across separate process
//! invocations (proving it is wall-clock invariant) and must move when an input
//! changes.

use std::path::PathBuf;
use std::process::Command;

fn labwired_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_labwired"))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn run_manifest_digest(script: &std::path::Path, out_dir: &std::path::Path) -> String {
    let output = Command::new(labwired_bin())
        .arg("test")
        .arg("--script")
        .arg(script)
        .arg("--output-dir")
        .arg(out_dir)
        .arg("--run-manifest")
        .arg("--no-uart-stdout")
        .output()
        .expect("failed to run labwired");

    let manifest_path = out_dir.join("run-manifest.json");
    assert!(
        manifest_path.exists(),
        "run-manifest.json not produced. exit {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    json["digest"]
        .as_str()
        .expect("manifest has a string digest")
        .to_string()
}

fn write_script(dir: &std::path::Path, firmware: &str, system: &str, max_steps: u32) -> PathBuf {
    let content = format!(
        "schema_version: \"1.0\"\ninputs:\n  firmware: \"{firmware}\"\n  system: \"{system}\"\nlimits:\n  max_steps: {max_steps}\nassertions: []\n"
    );
    let path = dir.join(format!("script-{max_steps}.yaml"));
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn manifest_digest_is_reproducible_and_input_sensitive() {
    let root = workspace_root();
    let firmware = root.join("tests/fixtures/uart-ok-thumbv7m.elf");
    let system = root.join("configs/systems/ci-fixture-uart1.yaml");
    if !firmware.exists() || !system.exists() {
        eprintln!("skipping manifest reproducibility test: fixtures absent");
        return;
    }
    let firmware = firmware.canonicalize().unwrap();
    let system = system.canonicalize().unwrap();

    let temp = std::env::temp_dir().join("labwired-manifest-reproducible");
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();

    let script = write_script(
        &temp,
        &firmware.display().to_string(),
        &system.display().to_string(),
        1000,
    );

    // Two separate process invocations (different wall-clock) -> same digest.
    let d1 = run_manifest_digest(&script, &temp.join("run_0"));
    let d2 = run_manifest_digest(&script, &temp.join("run_1"));
    assert_eq!(d1, d2, "manifest digest must be reproducible across runs");
    assert_eq!(d1.len(), 64, "digest must be a 64-char hex SHA-256");

    // A changed input (different limit -> different steps + config hash) moves it.
    let script2 = write_script(
        &temp,
        &firmware.display().to_string(),
        &system.display().to_string(),
        500,
    );
    let d3 = run_manifest_digest(&script2, &temp.join("run_2"));
    assert_ne!(d1, d3, "a changed input must change the digest");
}

/// Run the manifest flow and return the parsed JSON (not just the digest).
fn run_manifest_json(script: &std::path::Path, out_dir: &std::path::Path) -> serde_json::Value {
    let output = Command::new(labwired_bin())
        .arg("test")
        .arg("--script")
        .arg(script)
        .arg("--output-dir")
        .arg(out_dir)
        .arg("--run-manifest")
        .arg("--no-uart-stdout")
        .output()
        .expect("failed to run labwired");
    let manifest_path = out_dir.join("run-manifest.json");
    assert!(
        manifest_path.exists(),
        "run-manifest.json not produced. exit {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap()
}

#[test]
fn noise_enabled_run_declares_seeded_nondeterminism() {
    let root = workspace_root();
    let firmware = root.join("tests/fixtures/uart-ok-thumbv7m.elf");
    let chip = root.join("configs/chips/stm32f401.yaml");
    if !firmware.exists() || !chip.exists() {
        eprintln!("skipping noise manifest test: fixtures absent");
        return;
    }

    let temp = std::env::temp_dir().join("labwired-manifest-noise");
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();

    // A system wiring an MPU6050 with noise_sigma on the F401's modeled I2C1.
    let system = temp.join("system-noise.yaml");
    std::fs::write(
        &system,
        format!(
            "name: noise-fixture\nchip: \"{}\"\nexternal_devices:\n  - id: imu\n    type: mpu6050\n    connection: i2c1\n    config:\n      noise_sigma: 0.02\n",
            chip.canonicalize().unwrap().display()
        ),
    )
    .unwrap();

    let script = write_script(
        &temp,
        &firmware.canonicalize().unwrap().display().to_string(),
        &system.display().to_string(),
        1000,
    );

    let man_a = run_manifest_json(&script, &temp.join("run_0"));
    let man_b = run_manifest_json(&script, &temp.join("run_1"));

    assert_eq!(
        man_a["nondeterminism"].as_str().unwrap(),
        "seeded(sensor-noise)",
        "noise-enabled run must declare seeded(sensor-noise)"
    );
    assert_eq!(
        man_a["digest"].as_str().unwrap(),
        man_b["digest"].as_str().unwrap(),
        "seeded noise must still replay bit-identically"
    );
}

#[test]
fn noise_free_run_still_declares_none() {
    let root = workspace_root();
    let firmware = root.join("tests/fixtures/uart-ok-thumbv7m.elf");
    let system = root.join("configs/systems/ci-fixture-uart1.yaml");
    if !firmware.exists() || !system.exists() {
        eprintln!("skipping noise-free manifest test: fixtures absent");
        return;
    }
    let temp = std::env::temp_dir().join("labwired-manifest-noise-free");
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let script = write_script(
        &temp,
        &firmware.canonicalize().unwrap().display().to_string(),
        &system.canonicalize().unwrap().display().to_string(),
        1000,
    );
    let man = run_manifest_json(&script, &temp.join("run_0"));
    assert_eq!(man["nondeterminism"].as_str().unwrap(), "none");
}
