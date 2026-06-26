//! `validation-report <tier1-matrix.json> <chip> [--json]` — print a chip's
//! provenanced model-validation report (markdown by default, JSON with --json).
use anyhow::{anyhow, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let matrix_path = args
        .get(1)
        .ok_or_else(|| anyhow!("usage: validation-report <tier1-matrix.json> <chip> [--json]"))?;
    let chip = args
        .get(2)
        .ok_or_else(|| anyhow!("usage: validation-report <tier1-matrix.json> <chip> [--json]"))?;
    let as_json = args.iter().any(|a| a == "--json");

    let matrix_json = std::fs::read_to_string(matrix_path)?;
    let report = validation_report::report_from_tier1_matrix(&matrix_json, chip)?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.to_markdown());
    }
    Ok(())
}
