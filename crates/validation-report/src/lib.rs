//! Model-validation report: aggregate labwired's scattered fidelity evidence into
//! one provenanced, auditable artifact per chip.
//!
//! proto.cat's claim is "the firmware verifiably runs" — which is only as good as
//! the fidelity of the silicon models it runs on. labwired already validates models
//! several ways (tier-1 raw-register-vs-TRM matrix, silicon reset-conformance in the
//! `hw-oracle` crate, SVD-derived register coverage, real vendor-stack boot in the
//! examples), but the evidence lives in separate files. This crate consolidates it
//! into a single `ModelValidationReport` that says, per peripheral, WHAT was checked
//! and against WHICH authority — so "validated model" is an audit trail, not a claim.
//!
//! Source #1 (here): the tier-1 coverage matrix (`docs/coverage/tier1-matrix.json`).
//! Designed so further authorities (hw-oracle reset registers, SVD coverage, QEMU/
//! Renode differential) attach as additional per-peripheral checks without reshaping
//! the report.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::collections::BTreeMap;

/// Outcome of one validation check on a peripheral model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// The model matched the authority.
    Pass,
    /// The model disagreed with the authority — a real fidelity gap.
    Fail,
    /// The peripheral is intentionally not covered by this authority.
    NotApplicable,
    /// In scope but no result recorded yet (a tracked gap, never a silent pass).
    Unrecorded,
}

impl CheckStatus {
    fn parse(s: &str) -> CheckStatus {
        match s {
            "pass" => CheckStatus::Pass,
            "fail" => CheckStatus::Fail,
            "na" => CheckStatus::NotApplicable,
            _ => CheckStatus::Unrecorded,
        }
    }
    fn label(self) -> &'static str {
        match self {
            CheckStatus::Pass => "PASS",
            CheckStatus::Fail => "FAIL",
            CheckStatus::NotApplicable => "n/a",
            CheckStatus::Unrecorded => "unrecorded",
        }
    }
}

/// One peripheral's validation against one authority, with provenance.
#[derive(Debug, Clone, Serialize)]
pub struct PeripheralValidation {
    pub peripheral: String,
    pub status: CheckStatus,
    /// Which authority the model was checked against (human-readable, citable).
    pub authority: String,
    /// Link/path to the run or capture that backs this result, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

/// Rolled-up counts + coverage for a chip's model validation.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub pass: usize,
    pub fail: usize,
    pub not_applicable: usize,
    pub unrecorded: usize,
    /// pass / applicable, where applicable = pass + fail + unrecorded (excludes n/a).
    pub coverage_pct: f64,
}

/// The provenanced model-validation report for a single chip.
#[derive(Debug, Clone, Serialize)]
pub struct ModelValidationReport {
    pub chip: String,
    pub peripherals: Vec<PeripheralValidation>,
    pub summary: Summary,
}

impl ModelValidationReport {
    fn summarize(peripherals: &[PeripheralValidation]) -> Summary {
        let mut pass = 0;
        let mut fail = 0;
        let mut not_applicable = 0;
        let mut unrecorded = 0;
        for p in peripherals {
            match p.status {
                CheckStatus::Pass => pass += 1,
                CheckStatus::Fail => fail += 1,
                CheckStatus::NotApplicable => not_applicable += 1,
                CheckStatus::Unrecorded => unrecorded += 1,
            }
        }
        let applicable = pass + fail + unrecorded;
        let coverage_pct = if applicable == 0 {
            0.0
        } else {
            (pass as f64) * 100.0 / (applicable as f64)
        };
        Summary {
            pass,
            fail,
            not_applicable,
            unrecorded,
            coverage_pct,
        }
    }

    /// A human-auditable markdown rendering of the report.
    pub fn to_markdown(&self) -> String {
        let s = &self.summary;
        let mut out = String::new();
        out.push_str(&format!("# Model validation — {}\n\n", self.chip));
        out.push_str(&format!(
            "Coverage: **{:.1}%** ({} pass / {} fail / {} unrecorded; {} n/a)\n\n",
            s.coverage_pct, s.pass, s.fail, s.unrecorded, s.not_applicable
        ));
        out.push_str("| Peripheral | Result | Authority | Evidence |\n");
        out.push_str("|---|---|---|---|\n");
        for p in &self.peripherals {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                p.peripheral,
                p.status.label(),
                p.authority,
                p.evidence.as_deref().unwrap_or("—"),
            ));
        }
        out
    }
}

/// One peripheral entry in the tier-1 coverage matrix JSON.
#[derive(serde::Deserialize)]
struct Tier1Entry {
    status: String,
    #[serde(default)]
    run_url: Option<String>,
}

const TIER1_AUTHORITY: &str = "tier-1: raw-register sequence vs vendor TRM";

/// Build a chip's validation report from the tier-1 coverage matrix
/// (`docs/coverage/tier1-matrix.json`): a `{ chip: { peripheral: { status, run_url? } } }`
/// map. Errors if the chip is absent — a missing chip is a gap to surface, never a
/// silently-empty "validated" report.
pub fn report_from_tier1_matrix(matrix_json: &str, chip: &str) -> Result<ModelValidationReport> {
    let matrix: BTreeMap<String, BTreeMap<String, Tier1Entry>> = serde_json::from_str(matrix_json)?;
    let entries = matrix
        .get(chip)
        .ok_or_else(|| anyhow!("chip '{chip}' is not in the tier-1 coverage matrix"))?;

    // BTreeMap iteration is sorted by peripheral name → stable, auditable order.
    let peripherals: Vec<PeripheralValidation> = entries
        .iter()
        .map(|(name, e)| PeripheralValidation {
            peripheral: name.clone(),
            status: CheckStatus::parse(&e.status),
            authority: TIER1_AUTHORITY.to_string(),
            evidence: e.run_url.clone(),
        })
        .collect();

    let summary = ModelValidationReport::summarize(&peripherals);
    Ok(ModelValidationReport {
        chip: chip.to_string(),
        peripherals,
        summary,
    })
}
