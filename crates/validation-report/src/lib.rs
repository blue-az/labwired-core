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
//! Each authority contributes a list of `PeripheralValidation` checks; a peripheral
//! can carry checks from several authorities. The summary derives ONE status per
//! distinct peripheral (Fail > Pass > Unrecorded > n/a) and reports coverage over
//! peripherals, not raw rows — so being validated twice doesn't inflate the score.
//!
//! Sources wired: tier-1 coverage matrix (`docs/coverage/tier1-matrix.json`) and the
//! SVD-derived register descriptors (`configs/peripherals/<chip>/*.yaml`). Designed so
//! further authorities (hw-oracle reset registers, vendor-stack boot, QEMU/Renode
//! differential) attach as more checks without reshaping the report.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

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
    /// Merge two checks on the SAME peripheral into a derived status. A single
    /// disagreement fails the peripheral; otherwise any pass validates it; an
    /// unrecorded check keeps it open; n/a only when nothing else applies.
    fn merge(self, other: CheckStatus) -> CheckStatus {
        use CheckStatus::*;
        match (self, other) {
            (Fail, _) | (_, Fail) => Fail,
            (Pass, _) | (_, Pass) => Pass,
            (Unrecorded, _) | (_, Unrecorded) => Unrecorded,
            _ => NotApplicable,
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
    /// Extra context for this check (e.g. "423 registers").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Rolled-up counts + coverage for a chip's model validation, over DISTINCT
/// peripherals (a peripheral validated by several authorities counts once).
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub pass: usize,
    pub fail: usize,
    pub not_applicable: usize,
    pub unrecorded: usize,
    /// pass / applicable, where applicable = pass + fail + unrecorded (excludes n/a).
    pub coverage_pct: f64,
    /// Distinct authorities that contributed at least one check.
    pub authorities: usize,
}

/// The provenanced model-validation report for a single chip.
#[derive(Debug, Clone, Serialize)]
pub struct ModelValidationReport {
    pub chip: String,
    pub peripherals: Vec<PeripheralValidation>,
    pub summary: Summary,
}

impl ModelValidationReport {
    /// Build a report from any set of authority checks. Rows are sorted by
    /// (peripheral, authority) for a stable, auditable artifact; the summary is
    /// derived over distinct peripherals.
    pub fn from_checks(chip: &str, mut checks: Vec<PeripheralValidation>) -> ModelValidationReport {
        checks.sort_by(|a, b| {
            a.peripheral
                .cmp(&b.peripheral)
                .then(a.authority.cmp(&b.authority))
        });
        let summary = Self::summarize(&checks);
        ModelValidationReport {
            chip: chip.to_string(),
            peripherals: checks,
            summary,
        }
    }

    fn summarize(checks: &[PeripheralValidation]) -> Summary {
        let mut derived: BTreeMap<&str, CheckStatus> = BTreeMap::new();
        let mut authorities: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for c in checks {
            authorities.insert(c.authority.as_str());
            derived
                .entry(c.peripheral.as_str())
                .and_modify(|s| *s = s.merge(c.status))
                .or_insert(c.status);
        }
        let (mut pass, mut fail, mut not_applicable, mut unrecorded) = (0, 0, 0, 0);
        for s in derived.values() {
            match s {
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
            authorities: authorities.len(),
        }
    }

    /// A human-auditable markdown rendering of the report.
    pub fn to_markdown(&self) -> String {
        let s = &self.summary;
        let mut out = String::new();
        out.push_str(&format!("# Model validation — {}\n\n", self.chip));
        out.push_str(&format!(
            "Coverage: **{:.1}%** ({} pass / {} fail / {} unrecorded; {} n/a) across {} \
             distinct peripherals, {} authorit{}\n\n",
            s.coverage_pct,
            s.pass,
            s.fail,
            s.unrecorded,
            s.not_applicable,
            s.pass + s.fail + s.not_applicable + s.unrecorded,
            s.authorities,
            if s.authorities == 1 { "y" } else { "ies" },
        ));
        out.push_str("| Peripheral | Result | Authority | Detail | Evidence |\n");
        out.push_str("|---|---|---|---|---|\n");
        for p in &self.peripherals {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                p.peripheral,
                p.status.label(),
                p.authority,
                p.detail.as_deref().unwrap_or("—"),
                p.evidence.as_deref().unwrap_or("—"),
            ));
        }
        out
    }
}

// ── Authority #1: tier-1 raw-register-vs-TRM coverage matrix ──────────────────

/// One peripheral entry in the tier-1 coverage matrix JSON.
#[derive(serde::Deserialize)]
struct Tier1Entry {
    status: String,
    #[serde(default)]
    run_url: Option<String>,
}

const TIER1_AUTHORITY: &str = "tier-1: raw-register sequence vs vendor TRM";

/// Checks from the tier-1 coverage matrix (`docs/coverage/tier1-matrix.json`): a
/// `{ chip: { peripheral: { status, run_url? } } }` map. Errors if the chip is
/// absent — a missing chip is a gap to surface, never a silently-empty report.
pub fn tier1_checks(matrix_json: &str, chip: &str) -> Result<Vec<PeripheralValidation>> {
    let matrix: BTreeMap<String, BTreeMap<String, Tier1Entry>> = serde_json::from_str(matrix_json)?;
    let entries = matrix
        .get(chip)
        .ok_or_else(|| anyhow!("chip '{chip}' is not in the tier-1 coverage matrix"))?;
    Ok(entries
        .iter()
        .map(|(name, e)| PeripheralValidation {
            peripheral: name.clone(),
            status: CheckStatus::parse(&e.status),
            authority: TIER1_AUTHORITY.to_string(),
            evidence: e.run_url.clone(),
            detail: None,
        })
        .collect())
}

/// Convenience: a tier-1-only report for one chip.
pub fn report_from_tier1_matrix(matrix_json: &str, chip: &str) -> Result<ModelValidationReport> {
    Ok(ModelValidationReport::from_checks(
        chip,
        tier1_checks(matrix_json, chip)?,
    ))
}

// ── Authority #2: SVD-derived register descriptors ────────────────────────────

#[derive(serde::Deserialize)]
struct SvdDescriptor {
    peripheral: String,
    #[serde(default)]
    registers: Vec<serde_yaml::Value>,
}

const SVD_AUTHORITY: &str = "CMSIS-SVD register map (vendor register layout)";

/// Checks from the SVD-derived peripheral descriptors in a directory
/// (`configs/peripherals/<chip>/*.yaml`). Each descriptor is a vendor-authoritative
/// register layout; presence with registers validates the model's INTERFACE (not its
/// dynamic behavior — that is what tier-1 / vendor-stack boot cover). A descriptor
/// with zero registers is `unrecorded`, not a silent pass.
pub fn svd_checks(peripherals_dir: &Path) -> Result<Vec<PeripheralValidation>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(peripherals_dir)
        .map_err(|e| anyhow!("reading {}: {e}", peripherals_dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        let desc: SvdDescriptor =
            serde_yaml::from_str(&text).map_err(|e| anyhow!("parsing {}: {e}", path.display()))?;
        let n = desc.registers.len();
        out.push(PeripheralValidation {
            peripheral: desc.peripheral.to_lowercase(),
            status: if n > 0 {
                CheckStatus::Pass
            } else {
                CheckStatus::Unrecorded
            },
            authority: SVD_AUTHORITY.to_string(),
            evidence: path
                .file_name()
                .and_then(|f| f.to_str())
                .map(str::to_string),
            detail: Some(format!("{n} register{}", if n == 1 { "" } else { "s" })),
        });
    }
    Ok(out)
}

// ── Authority #3: silicon reset-conformance (hw-oracle OpenOCD captures) ───────

#[derive(serde::Deserialize)]
struct ResetCapture {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    blocks: BTreeMap<String, ResetBlock>,
}

#[derive(serde::Deserialize)]
struct ResetBlock {
    #[serde(default)]
    words: BTreeMap<String, String>,
}

const HW_ORACLE_AUTHORITY: &str = "silicon reset-conformance (OpenOCD capture vs real hardware)";

/// Checks from a committed hw-oracle reset capture
/// (`scripts/hw-oracle/captures/<chip>/.../reg_oracle.json`): real-silicon reset
/// register values read over OpenOCD from a physical board. Per peripheral block, the
/// number of registers with real-hardware ground truth the `hw-oracle` conformance
/// suite diffs the model against. This is the strongest authority — the model is held
/// to values measured on actual silicon — and needs no hardware at check time (the
/// capture is committed). A block with no words is `unrecorded`, never a silent pass.
pub fn hw_oracle_checks(capture_json: &str) -> Result<Vec<PeripheralValidation>> {
    let capture: ResetCapture = serde_json::from_str(capture_json)?;
    let source = capture.source;
    Ok(capture
        .blocks
        .into_iter()
        .map(|(name, block)| {
            let n = block.words.len();
            PeripheralValidation {
                peripheral: name.to_lowercase(),
                status: if n > 0 {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Unrecorded
                },
                authority: HW_ORACLE_AUTHORITY.to_string(),
                evidence: source.clone(),
                detail: Some(format!(
                    "{n} reset register{} vs real silicon",
                    if n == 1 { "" } else { "s" }
                )),
            }
        })
        .collect())
}
