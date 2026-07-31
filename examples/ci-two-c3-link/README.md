# Engine fixture: two ESP32-C3s on one UART wire

The smallest thing that proves cross-chip serial works on a GPIO-matrix chip.
Two independent C3 nodes, cross-linked on UART1, exchanging `PING` / `PONG`.

This is a **test fixture, not a lab**. The user-facing version — Arduino, with
the rally drawn on an OLED — is `examples/esp32c3-pingpong`.

It exists separately because CI must be able to run it with nothing installed:
the firmware is bare-metal Rust for `riscv32imc-unknown-none-elf`, a target that
ships with stock rustup. No Espressif toolchain, no ESP-IDF, no PlatformIO
builder, no network. The Arduino sketches cannot meet that bar, because
compiling them needs the hosted builder.

Bare-metal is also the point: the firmware writes the C3's UART registers
directly, so a regression in the `esp_uart` model surfaces here instead of being
absorbed by a HAL.

```
cargo build --release          # writes firmware/{server,client}.elf
```

The assertions live in `crates/core/tests/world_esp32c3_pingpong.rs`.
