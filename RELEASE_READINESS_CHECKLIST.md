# Release Readiness Checklist (v0.14.0)

**Date**: 2026-05-12
**Version**: 0.14.0
**Coordinator**: @w1ne

## 1. Documentation Audit
- [x] **Changelog Updated**: `CHANGELOG.md` captures major changes since the previous release.
- [x] **Install Docs Updated**: `README.md` pinned install examples reference `v0.14.0`.
- [x] **Process Docs Updated**: `RELEASE_PROCESS.md` uses neutral version examples instead of stale historical versions.

## 2. Codebase Integrity
- [x] **Cargo Check**: `cargo check` passes after the workspace version bump.
- [x] **Tests Passing**: `cargo test --workspace` passes outside the sandbox; the sandboxed run cannot bind the GDB E2E localhost socket.
- [x] **Formatting**: `cargo fmt --all -- --check` passes.
- [x] **Diff Hygiene**: `git diff --check` reports no whitespace errors.

## 3. Artifacts & Packaging
- [x] **Version Bump**: `Cargo.toml` and `Cargo.lock` resolve workspace-managed crates to `0.14.0`.
- [x] **Generated Output Cleanup**: Tracked `out/**` run artifacts and accidental build products are removed; committed test fixtures remain.
- [x] **Release Notes Prepared**: `CHANGELOG.md` `0.14.0` section is ready to publish as the GitHub release body.

## 4. Final Review
- [x] **Working Tree Reviewed**: Release-prep diff contains expected metadata, documentation, generated-output cleanup, formatter changes, and the stale I2C fidelity test fix.
- [x] **Release Notes Reviewed**: GitHub release body matches the `CHANGELOG.md` `0.14.0` section.

---
**Status**: [x] READY FOR RELEASE

## Known Issues (v0.14.0)
- No release-blocking known issues documented at prep time.
