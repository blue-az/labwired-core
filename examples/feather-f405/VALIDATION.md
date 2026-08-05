# Feather-STM32F405 Validation Runbook

Run all commands from `core/`.

## 0) Firmware reuse rationale (and its invalidation conditions)

The smoke firmware is `firmware-f401-demo`, reused unchanged. The reuse is
valid because the F405 descriptor and the F401 descriptor agree on every
address the firmware touches: Cortex-M4F core, flash base `0x08000000`,
SRAM base `0x20000000`, USART2 at `0x40004400` on IRQ 38, GPIOA/GPIOC bases,
RCC `stm32f4` profile. It would be invalidated by: an F405-specific memory.x
(CCM at `0x10000000`), F405-only peripheral use (I2C2/SPI2/USART3/TIM3-5 are
declared follow-ups), or any silicon delta found against F405 hardware.

## 1) Optional: ensure target installed

```bash
rustup target add thumbv7em-none-eabi
```

## 2) Build smoke firmware

```bash
cargo build -p firmware-f401-demo --release --target thumbv7em-none-eabi
```

## 3) Run deterministic UART smoke

```bash
cargo run -q -p labwired-cli -- test \
  --script examples/feather-f405/uart-smoke.yaml \
  --output-dir out/feather-f405/uart-smoke \
  --no-uart-stdout
```

Pass criteria:
1. exit code is `0`
2. UART contains `OK`

## 4) Run direct simulation for PC/SP evidence

```bash
cargo run -q -p labwired-cli -- \
  --firmware target/thumbv7em-none-eabi/release/firmware-f401-demo \
  --system configs/systems/feather-f405.yaml \
  --max-steps 32 \
  --json
```

## 5) Run unsupported-instruction audit

```bash
./scripts/unsupported_instruction_audit.sh \
  --firmware target/thumbv7em-none-eabi/release/firmware-f401-demo \
  --system configs/systems/feather-f405.yaml \
  --max-steps 200000 \
  --out-dir out/unsupported-audit/feather-f405
```

Pass criteria:
1. script exits `0`
2. audit report exists at `out/unsupported-audit/feather-f405/report.md`

## Validation record

- 2026-08-04: steps 2–5 executed on the feat/sim86-parity-pack branch —
  UART smoke exit 0 (`OK` received), direct sim `status: finished` (32
  steps), unsupported-instruction audit `unsupported_total: 0`, report at
  `out/unsupported-audit/feather-f405/report.md`.
