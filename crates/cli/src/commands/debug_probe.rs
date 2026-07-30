// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.

//! Scripted debug probe for agents (v1): one-shot run-to-stop with optional breakpoints.
//! Output is agent-friendly JSON; never claims oracle proof.

use serde::{Deserialize, Serialize};
use serde_json::json;

/// Breakpoint request shapes accepted by the debug probe (JSON untagged).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum BreakpointSpec {
    Address { address: String },
    Symbol { symbol: String },
    Line {
        line: u32,
        #[serde(default)]
        file: Option<String>,
    },
}

/// Resolution outcome for one requested breakpoint.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BreakpointOutcome {
    pub requested: serde_json::Value,
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Agent-facing JSON result for a debug probe run.
///
/// Observational only: `proven` is always `false`.
#[derive(Debug, Clone, Serialize)]
pub struct DebugProbeResult {
    pub status: String, // "ok" | "error"
    pub stop_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycles: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default)]
    pub breakpoints: Vec<BreakpointOutcome>,
    /// Always false — observational probe only.
    pub proven: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
}

impl DebugProbeResult {
    /// Build an error result. Never sets `proven`.
    pub fn error(code: &str, detail: &str) -> Self {
        Self {
            status: "error".into(),
            stop_reason: "config_error".into(),
            pc: None,
            cycles: None,
            location: None,
            registers: None,
            serial: None,
            breakpoints: vec![],
            proven: false,
            error: Some(json!({ "code": code, "detail": detail })),
        }
    }
}

/// Parse an address string the same way as CLI `parse_u32_addr`:
/// optional `0x`/`0X` hex, otherwise decimal. No underscore separators.
pub fn parse_address(s: &str) -> Option<u32> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        t.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_address_hex_and_decimal() {
        assert_eq!(parse_address("0x40000000"), Some(0x4000_0000));
        assert_eq!(parse_address("0XABCD"), Some(0xABCD));
        assert_eq!(parse_address("1234"), Some(1234));
        assert_eq!(parse_address("  0x10  "), Some(0x10));
        assert_eq!(parse_address("nope"), None);
        assert_eq!(parse_address(""), None);
        // Underscores are not supported (match main.rs parse_u32_addr).
        assert_eq!(parse_address("0x4000_0000"), None);
    }

    #[test]
    fn error_result_never_proven() {
        let r = DebugProbeResult::error("NO_FIRMWARE", "missing elf");
        assert_eq!(r.proven, false);
        assert_eq!(r.status, "error");
        assert_eq!(r.stop_reason, "config_error");
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["proven"], false);
        assert_eq!(v["error"]["code"], "NO_FIRMWARE");
        assert_eq!(v["error"]["detail"], "missing elf");
    }

    #[test]
    fn breakpoint_spec_untagged_deser() {
        let addr: BreakpointSpec =
            serde_json::from_str(r#"{"address":"0x08000100"}"#).unwrap();
        assert_eq!(
            addr,
            BreakpointSpec::Address {
                address: "0x08000100".into()
            }
        );

        let sym: BreakpointSpec = serde_json::from_str(r#"{"symbol":"main"}"#).unwrap();
        assert_eq!(
            sym,
            BreakpointSpec::Symbol {
                symbol: "main".into()
            }
        );

        let line: BreakpointSpec =
            serde_json::from_str(r#"{"line":42,"file":"main.c"}"#).unwrap();
        assert_eq!(
            line,
            BreakpointSpec::Line {
                line: 42,
                file: Some("main.c".into())
            }
        );

        let line_only: BreakpointSpec = serde_json::from_str(r#"{"line":7}"#).unwrap();
        assert_eq!(
            line_only,
            BreakpointSpec::Line {
                line: 7,
                file: None
            }
        );
    }

    #[test]
    fn breakpoint_outcome_serializes_optional_fields() {
        let verified = BreakpointOutcome {
            requested: json!({"address":"0x1000"}),
            verified: true,
            address: Some("0x00001000".into()),
            message: None,
        };
        let v = serde_json::to_value(&verified).unwrap();
        assert_eq!(v["verified"], true);
        assert_eq!(v["address"], "0x00001000");
        assert!(v.get("message").is_none());

        let unverified = BreakpointOutcome {
            requested: json!({"symbol":"foo"}),
            verified: false,
            address: None,
            message: Some("symbol not found".into()),
        };
        let v = serde_json::to_value(&unverified).unwrap();
        assert_eq!(v["verified"], false);
        assert!(v.get("address").is_none());
        assert_eq!(v["message"], "symbol not found");
    }
}
