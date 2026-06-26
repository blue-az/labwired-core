//! `validation-report <tier1-matrix.json> <chip> [peripherals-dir] [--json]`
//!
//! Prints a chip's provenanced model-validation report (markdown by default, JSON
//! with --json). Aggregates the tier-1 matrix plus, when a peripherals descriptor
//! directory is given (or auto-found at `configs/peripherals/<chip>`), the SVD
//! register-layout authority.
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use validation_report::{svd_checks, tier1_checks, ModelValidationReport};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let positional: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with("--")).collect();
    let matrix_path = positional.first().ok_or_else(|| {
        anyhow!("usage: validation-report <tier1-matrix.json> <chip> [peripherals-dir] [--json]")
    })?;
    let chip = positional.get(1).ok_or_else(|| {
        anyhow!("usage: validation-report <tier1-matrix.json> <chip> [peripherals-dir] [--json]")
    })?;
    let as_json = args.iter().any(|a| a == "--json");

    let matrix_json = std::fs::read_to_string(matrix_path)?;
    let mut checks = tier1_checks(&matrix_json, chip)?;

    // SVD authority: explicit dir, else auto-find configs/peripherals/<chip>.
    let svd_dir = positional
        .get(2)
        .map(|p| PathBuf::from(p.as_str()))
        .unwrap_or_else(|| PathBuf::from(format!("configs/peripherals/{chip}")));
    if svd_dir.is_dir() {
        checks.extend(svd_checks(&svd_dir)?);
    }

    let report = ModelValidationReport::from_checks(chip, checks);
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.to_markdown());
    }
    Ok(())
}
