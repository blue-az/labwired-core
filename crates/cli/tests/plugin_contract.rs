//! End-to-end contract for the chip-plugin seam.
//!
//! Uses only the public APIs of `labwired-cli`, `labwired-core`, and
//! `labwired-config` — the same surface the private `labwired-ip` repo will.

use labwired_core::plugin::{ChipPlugin, PLUGIN_API_VERSION};

struct ContractPlugin;

impl ChipPlugin for ContractPlugin {
    fn api_version(&self) -> u32 {
        PLUGIN_API_VERSION
    }
    fn chip_names(&self) -> &[&str] {
        &["contract-chip"]
    }
    fn chip_yaml(&self, name: &str) -> Option<&'static str> {
        (name == "contract-chip").then_some(
            "name: \"contract-chip\"\narch: \"arm\"\ncore: \"cortex-m0+\"\n\
             flash: { base: 0, size: \"4KB\" }\n\
             ram: { base: 0x20000000, size: \"1KB\" }\nperipherals: []\n",
        )
    }
}

#[test]
fn plugin_chip_resolves_by_bare_name() {
    let plugins: [&dyn ChipPlugin; 1] = [&ContractPlugin];
    let d = labwired_config::ChipDescriptor::resolve_with(
        "contract-chip",
        std::path::Path::new("."),
        &|name| plugins.iter().find_map(|p| p.chip_yaml(name)),
    )
    .unwrap();
    assert_eq!(d.name, "contract-chip");
}

#[test]
fn version_mismatch_is_refused() {
    struct Stale;
    impl ChipPlugin for Stale {
        fn api_version(&self) -> u32 {
            PLUGIN_API_VERSION + 1
        }
    }
    // Use the extracted gate so we don't launch the full CLI (which would
    // parse argv and try to run a subcommand).
    let err = labwired_cli::check_plugin_versions(&[&Stale]).unwrap_err();
    assert!(
        err.contains("plugin API mismatch"),
        "expected mismatch message, got: {err}"
    );
    assert!(
        err.contains(&format!("v{}", PLUGIN_API_VERSION + 1)),
        "expected stale version in message, got: {err}"
    );
}
