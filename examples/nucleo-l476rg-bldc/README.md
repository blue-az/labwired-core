# STM32L476 six-step BLDC control

This example runs real bare-metal STM32L476RG firmware against LabWired's
deterministic BLDC plant. TIM1 supplies three main/complementary PWM pairs with
dead time and master-output gating; PA0 is the external inverter enable.

The plant uses 24 V, 0.35 Ω phase resistance, 180 µH phase inductance,
0.04 N·m/A torque constant, 0.04 V/(rad/s) back-EMF constant, 20 µkg·m² rotor
inertia, seven pole pairs, and a 2048-count encoder. Hall A/B/C feed PA1..PA3;
encoder A/B/index feed PA4..PA6; separate motor and inverter faults feed PA7
and PB7.

The firmware begins with open-loop duty, selects a six-step row from the Hall
sequence `001 → 101 → 100 → 110 → 010 → 011`, and uses a deliberately small
bounded proportional regulator to adjust duty from Hall edge period. Invalid
Hall and stationary counters are bounded. Every fault handler first clears
TIM1 `BDTR.MOE`, disables all CCER legs, and drops PA0 before printing.

```bash
cargo build --release -p firmware-l476-demo \
  --bin firmware-l476-bldc-six-step --target thumbv7em-none-eabihf
cargo run -q -p labwired-cli -- test \
  --script examples/ci/l476-bldc-stall.yaml \
  --output-dir out/l476-bldc
```

The UART sequence is `BLDC READY`, `TARGET REACHED`, `FAULT STALL`, and
`INVERTER OFF`. The CI script injects a real mechanical stall through the
generic motor input after target acquisition; it does not forge UART output.
The run has a 100,000-cycle (1.25 ms) post-injection ceiling; the reference run
shuts down well inside that bound (the acceptance artifact records the exact
cycle count for each run).

For robotics startups this moves repeatable commutation, startup, Hall-order,
and shutdown regressions into CI while retaining the same MCU binary and
register-level safety path used on hardware.
