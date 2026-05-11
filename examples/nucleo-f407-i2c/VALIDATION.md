# NUCLEO-F407 hardware-validation log

Every commit to the F407 chip yaml or any peripheral that F407 firmware
touches must keep the survival tests green. This file is the audit
trail: which traces have been captured against real silicon, what
revealed each bug, and which simulator commits closed each gap.

Mirrors the workflow already proven on
[`docs/boards/nucleo-l476rg.md`](../../docs/boards/nucleo-l476rg.md).

## Hardware

- Board: **NUCLEO-F407** (or STM32F4-DISCO with an external USB-UART
  on PA2/PA3 for survival traces; the I²C lane assumes Nucleo's
  on-board ST-LINK Virtual COM Port).
- Debugger: on-board ST-LINK V2-1.
- Host: Linux, `arm-none-eabi-gcc 14.x`, OpenOCD 0.12+.
- DBGMCU IDCODE @ 0xE0042000 = (to be filled by Round 1 capture).
  The chip yaml currently encodes `0x10070413` as a placeholder.

## Survival traces

Each row is a captured byte stream that the simulator must reproduce
byte-for-byte (`crates/core/tests/firmware_survival.rs::test_nucleo_f407_*`).

| Trace                   | Fixture ELF                                     | Hardware capture file                                  | Status                          |
|-------------------------|-------------------------------------------------|--------------------------------------------------------|---------------------------------|
| `nucleo_f407_smoke`     | `tests/fixtures/nucleo-f407-smoke.elf`          | `tests/fixtures/hw_traces/nucleo_f407_smoke.txt` (pending) | Sim-only, Round 1 capture pending |
| `nucleo_f407_i2c`       | (to land — `firmware-f407-demo` second binary)  | `tests/fixtures/hw_traces/nucleo_f407_i2c.txt` (pending)   | Not yet built                   |

## Capture-session playbook

For each trace, the bench-side workflow is:

1. **Build the firmware** (host side, no hardware needed):
   ```bash
   cargo build --release -p firmware-f407-demo
   ```
   Output: `target/thumbv7em-none-eabi/release/firmware-f407-smoke`.

2. **Stage the ELF as a test fixture**:
   ```bash
   cp target/thumbv7em-none-eabi/release/firmware-f407-smoke \
      tests/fixtures/nucleo-f407-smoke.elf
   ```
   (Already done on first round; re-do after every firmware change.)

3. **Run the sim-only assertion** to lock in the expected output:
   ```bash
   cargo test -p labwired-core --test firmware_survival \
       test_nucleo_f407_smoke_survival --release
   ```
   This must pass with the current `expected_uart_output` literal in
   `SURVIVAL_CASES` before flashing — it pins the simulator behavior.

4. **Flash the firmware to silicon**:
   ```bash
   openocd -f interface/stlink.cfg -f target/stm32f4x.cfg \
       -c "program tests/fixtures/nucleo-f407-smoke.elf verify reset exit"
   ```

5. **Capture the Virtual COM Port output**:
   ```bash
   stty -F /dev/ttyACM0 115200 cs8 -cstopb -parenb -echo raw
   timeout 3 cat /dev/ttyACM0 > tests/fixtures/hw_traces/nucleo_f407_smoke.txt
   ```
   Reset the board (NRST button on the Nucleo) once during the
   3-second window. The smoke firmware prints its payload then halts
   in `wfi`, so the byte stream is finite.

6. **Diff the silicon trace against `expected_uart_output`**:
   ```bash
   diff <(xxd tests/fixtures/hw_traces/nucleo_f407_smoke.txt) \
        <(printf 'F407 SMOKE\r\nDEV=...\r\nMUL=...\r\nDONE\r\n' | xxd)
   ```
   If they match → the trace is silicon-validated, commit the
   `hw_traces/` file as the audit artifact. If they diverge → that's
   the bug. Investigate, fix the simulator (or the chip yaml), update
   `expected_uart_output` to match silicon, re-run step 3.

## Rounds

Each round below records a sim↔silicon divergence the survival trace
surfaced and the simulator commit that closed it. Empty rounds mean
"hardware capture still pending."

### Round 1 — UART smoke (`nucleo_f407_smoke`)

**Capture attempted 2026-05-11.** Hardware: STM32F407 board, standalone
ST-LINK V2 (USB ID `0483:3748`). OpenOCD program/verify succeeded.

**Divergence #1 — DBGMCU REV_ID** (closed).
OpenOCD reported `device id = 0x10016413` from the silicon. The chip
yaml placeholder was `0x10070413` (REV_ID `0x1007`). Real silicon is
REV_ID `0x1001` ("Rev 1", the most common revision for F407V/Z/IG).
Updated `configs/chips/stm32f407.yaml::dbgmcu.config.idcode` to
`0x10016413` and the survival expected_uart_output to
`DEV=10016413`. Sim test passes.

**UART capture itself: blocked by hardware setup.** The ST-LINK V2
attached is the standalone debugger probe — it provides SWD only, no
USB-CDC Virtual COM Port. (Nucleo-F407 boards have ST-LINK V2-**1**
with USB ID `0483:374b`, which exposes the USB-CDC bridge to USART2.)
The visible `/dev/ttyACM*` device on this host turned out to be an
ESP32-S3 from a different project.

To complete the smoke capture (Round 1 follow-up), need one of:
- **USB-UART cable** wired to PA2 (TX) / PA3 (RX) / GND. Then
  re-attempt the `cat /dev/ttyACM* > hw_traces/nucleo_f407_smoke.txt`
  step with the cable's tty.
- **Switch to a Nucleo-F407** board (has ST-LINK V2-1 with USB-CDC).
- **SWO trace via openocd** as an alternative — would require
  rewriting the smoke firmware to emit via ITM port 0 instead of
  USART2, then capturing through `openocd -c "tpiu config internal ...".

Other things worth re-checking once the line is up:
- **RCC bring-up timing.** The smoke firmware doesn't touch the PLL,
  so silicon stays on HSI 16 MHz like the simulator. If a future
  round adds a clock-tree exercise the BRR computation needs to be
  re-derived for the new SYSCLK.
- **F4 USART_SR vs L4 USART_ISR.** This firmware uses the classic
  F4 layout (SR/DR at offsets 0/4). If silicon UART output is silent
  or garbled, check that the chip yaml's USART2 type dispatches the
  V1 register layout (not V2).

### Round 2 — I²C state machine (`nucleo_f407_i2c`)

**Not yet built.** Will be a `firmware-f407-i2c` binary that drives the
AHT20 + BMP280 transactions via UART-traced events (e.g.
`I2C START\r\nADDR=ED\r\nDR=58\r\n`) so the survival diff catches
state-machine divergences. The `crates/core/src/peripherals/i2c.rs`
fixes that landed in commit `63b3f03` should pre-emptively cover the
common cases; the trace will tell us if anything else is hiding.
