# NUCLEO-F767ZI Validation Runbook

Run all commands from `core/`.

## 0) Firmware reuse rationale (and its invalidation conditions)

The smoke firmware is `firmware-f401-demo`, reused unchanged. Valid because:
Thumb-2/v7em is a subset of the M7's ISA, flash base `0x08000000` and SRAM
base `0x20000000` match, USART2 sits at `0x40004400` on IRQ 38 on both, and
the F7 RCC covers the F4 bring-up registers this firmware touches. The reuse
would be invalidated by: firmware configuring PLLSAI/DCKCFGR muxes (not
modeled), F7-only peripherals, or any silicon delta found against an F767
bench part (none exists yet — this chip is NOT_SHIPPED in the ratchet).

## 1) Build smoke firmware

```bash
rustup target add thumbv7em-none-eabi   # once
cargo build -p firmware-f401-demo --release --target thumbv7em-none-eabi
```

## 2) Run deterministic UART smoke

```bash
cargo run -q -p labwired-cli -- test \
  --script examples/nucleo-f767zi/uart-smoke.yaml \
  --output-dir out/nucleo-f767zi/uart-smoke \
  --no-uart-stdout
```

Pass criteria: exit `0`, UART contains `OK`.

## 3) Run unsupported-instruction audit

```bash
./scripts/unsupported_instruction_audit.sh \
  --firmware target/thumbv7em-none-eabi/release/firmware-f401-demo \
  --system examples/nucleo-f767zi/system.yaml \
  --max-steps 200000 \
  --out-dir out/unsupported-audit/nucleo-f767zi
```

Pass criteria: exit `0`, report exists.

## Validation record

- 2026-08-05: UART smoke exit 0 (`OK` received) on the feat/stm32f767 branch;
  audit result recorded below once run.
