# validation-report — provenanced model-fidelity reports

proto.cat's differentiator is *real working hardware*: "the firmware verifiably runs."
That claim is only as good as the **fidelity of the silicon models** it runs on — so
"validated model" has to be an **audit trail**, not an assertion.

labwired already validates models several ways, but the evidence is scattered:

| Authority | Where it lives | What it proves |
|---|---|---|
| tier-1 raw-register vs TRM | `docs/coverage/tier1-matrix.json` | each peripheral's register sequence matches the vendor TRM |
| silicon reset-conformance | `crates/hw-oracle/` (committed silicon captures) | reset-state registers match real silicon, no board needed at check time |
| SVD register coverage | `crates/svd-ingestor/` + `configs/peripherals/` | register map is vendor-authoritative (CMSIS-SVD) |
| real vendor-stack boot | `examples/*/VALIDATION.md` | unmodified ESP-IDF/Zephyr/HAL/UDSLib runs correctly |

This crate consolidates that into one `ModelValidationReport` per chip: per peripheral,
**what** was checked and against **which authority**, with a link/path to the backing
run. So a reviewer (or proto.cat's verdict) can cite validation, not just claim it.

## Use

```sh
cargo run -p validation-report -- docs/coverage/tier1-matrix.json esp32c3        # markdown
cargo run -p validation-report -- docs/coverage/tier1-matrix.json esp32c3 --json # json
```

## Status / roadmap

- **Now:** aggregates the tier-1 matrix (source #1). Coverage = pass / applicable
  (excludes `n/a`); `n/a`/`unrecorded` peripherals are shown, never silently dropped.
- **Next authorities** plug in as additional per-peripheral checks (the report shape is
  built for it): hw-oracle reset-conformance counts, SVD register coverage, and a
  vendor-stack-boot pass/fail derived from the examples.
- **Later (needs infra):** QEMU (Espressif fork) / Renode **differential** — run the same
  firmware on labwired and the reference emulator, diff traces, attach as another column.
  No hardware required; deferred until a runner exists (same posture as on-silicon HIL).
