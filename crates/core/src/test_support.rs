// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
// SPDX-License-Identifier: MIT

//! Shared helpers for the workspace's test binaries.
//!
//! Not part of the simulator's public API.

use std::path::{Path, PathBuf};

/// Cargo's target directory for the currently running test binary.
///
/// **The target directory is not always `<workspace>/target`.** A
/// `CARGO_TARGET_DIR` env var or a `[build] target-dir` in
/// `.cargo/config.toml` moves it, which is an ordinary thing to do when the
/// build tree lives on another disk — and this repo does exactly that, one
/// bucket per worktree, so parallel builds don't collide.
///
/// Tests that hardcoded `<manifest>/../../target` broke in two different ways
/// when that happened, because the child `cargo build` they shell out to
/// *inherits* the real target dir and writes the artifact somewhere else:
///
/// * the test that asserts on the ELF failed with "ELF not found after build",
///   pointing at a path the build was never going to write, and
/// * every test that instead does `if !elf.exists() { return }` **silently
///   passed without testing anything** — which is worse, because the suite
///   still reported green.
///
/// See [`resolve_target_dir`] for the resolution order.
pub fn target_dir() -> PathBuf {
    resolve_target_dir(
        std::env::var_os("CARGO_TARGET_TMPDIR").map(PathBuf::from),
        std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from),
        std::env::current_exe().ok().as_deref(),
        &workspace_default_target_dir(),
    )
}

/// The resolution itself, with every input passed in so it can be tested
/// without touching process-wide environment state.
///
/// First hit wins:
///
/// 1. `CARGO_TARGET_TMPDIR`'s parent. Cargo sets this for every *integration*
///    test, and it is the only signal that survives a `[build] target-dir` in
///    `.cargo/config.toml` — that form never reaches the test as an env var.
/// 2. `CARGO_TARGET_DIR`. Unit tests (`src/`) get no `CARGO_TARGET_TMPDIR`,
///    so this is what covers them when the env var is what moved the dir.
/// 3. The test executable's own location, `<target>/<profile>/deps/<bin>`, for
///    unit tests under a `[build] target-dir`.
/// 4. `default` — the `<workspace>/target` layout, unchanged behaviour.
fn resolve_target_dir(
    tmpdir: Option<PathBuf>,
    target_dir_env: Option<PathBuf>,
    current_exe: Option<&Path>,
    default: &Path,
) -> PathBuf {
    if let Some(dir) = tmpdir.as_deref().and_then(Path::parent) {
        return dir.to_path_buf();
    }
    if let Some(dir) = target_dir_env {
        return dir;
    }
    if let Some(dir) = current_exe.and_then(target_dir_from_exe_path) {
        return dir;
    }
    default.to_path_buf()
}

/// `<target>/<profile>/deps/<test-binary>` → `<target>`.
///
/// Only trusted when the executable really does sit in a `deps/` directory;
/// anything else (a runner that copied the binary elsewhere) returns `None`
/// so the caller falls through to the default layout rather than guessing.
fn target_dir_from_exe_path(exe: &Path) -> Option<PathBuf> {
    let deps = exe.parent()?;
    if deps.file_name()? != "deps" {
        return None;
    }
    deps.parent()?.parent().map(PathBuf::from)
}

/// The layout cargo uses when nothing has moved it: `<workspace>/target`.
fn workspace_default_target_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = "/workspace/target";

    fn resolve(tmpdir: Option<&str>, target_dir_env: Option<&str>, exe: Option<&str>) -> PathBuf {
        resolve_target_dir(
            tmpdir.map(PathBuf::from),
            target_dir_env.map(PathBuf::from),
            exe.map(Path::new),
            Path::new(DEFAULT),
        )
    }

    /// The regression this module exists for. A bucketed build must NOT
    /// resolve to `<workspace>/target`: the child `cargo build` writes the
    /// firmware into the bucket, so a test looking at the default layout
    /// finds nothing — and most callers respond to "nothing" by skipping.
    #[test]
    fn a_bucketed_target_dir_never_resolves_to_the_workspace_default() {
        for (tmpdir, env, exe) in [
            (Some("/bucket/tmp"), None, None),
            (None, Some("/bucket"), None),
            (None, None, Some("/bucket/debug/deps/core-abc123")),
            // All three present: they agree, and precedence must not matter.
            (
                Some("/bucket/tmp"),
                Some("/bucket"),
                Some("/bucket/debug/deps/core-abc123"),
            ),
        ] {
            let got = resolve(tmpdir, env, exe);
            assert_eq!(
                got,
                PathBuf::from("/bucket"),
                "tmpdir={tmpdir:?} env={env:?} exe={exe:?}"
            );
            assert_ne!(got, PathBuf::from(DEFAULT));
        }
    }

    /// `CARGO_TARGET_TMPDIR` wins, because it is the only input that tracks a
    /// `[build] target-dir` in `.cargo/config.toml` — that form never reaches
    /// the test as an env var, so a stale `CARGO_TARGET_DIR` must not beat it.
    #[test]
    fn cargo_target_tmpdir_outranks_the_env_var_and_the_exe_path() {
        assert_eq!(
            resolve(
                Some("/from-tmpdir/tmp"),
                Some("/from-env"),
                Some("/from-exe/debug/deps/core-abc123"),
            ),
            PathBuf::from("/from-tmpdir"),
        );
        // …and the env var still outranks the exe path.
        assert_eq!(
            resolve(
                None,
                Some("/from-env"),
                Some("/from-exe/debug/deps/core-abc")
            ),
            PathBuf::from("/from-env"),
        );
    }

    /// With nothing moved, behaviour is exactly what it was before this
    /// module existed. This is the "don't break the default" direction — the
    /// bucketed assertions above prove nothing on their own.
    #[test]
    fn an_unmoved_build_still_resolves_to_the_workspace_default() {
        assert_eq!(resolve(None, None, None), PathBuf::from(DEFAULT));
    }

    /// A binary that is not in `deps/` is not evidence about the target dir.
    /// Guessing from it would silently point tests at a sibling directory.
    #[test]
    fn an_exe_outside_deps_is_ignored_rather_than_guessed_from() {
        assert_eq!(
            resolve(None, None, Some("/somewhere/else/core-abc123")),
            PathBuf::from(DEFAULT),
        );
        assert_eq!(target_dir_from_exe_path(Path::new("/bare")), None);
    }

    /// The live wiring, not just the pure function: whatever cargo did to
    /// this run, the answer must contain the binary cargo is executing.
    #[test]
    fn the_live_resolution_contains_this_test_binary() {
        let exe = std::env::current_exe().expect("current_exe");
        let resolved = target_dir();
        let resolved = resolved.canonicalize().unwrap_or(resolved);
        let exe = exe.canonicalize().unwrap_or(exe);
        assert!(
            exe.starts_with(&resolved),
            "resolved target dir {resolved:?} does not contain the running binary {exe:?}"
        );
    }
}
