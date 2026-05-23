# labwired-ereader (Arduino-ESP32 reference)

Working firmware for the LabWired e-reader demo board.

## Hardware
- ESP32-WROOM-32 (any module — AgentDeck unit verified)
- Waveshare 2.9" tri-color e-paper (**C90c driver IC**)

Wiring (Arduino-ESP32 default VSPI pins):

| Pin | ESP32 GPIO |
|---|---|
| CS  | GPIO5 |
| DC  | GPIO17 |
| RST | GPIO16 |
| BUSY| GPIO4 |
| SCK | GPIO18 |
| MOSI (DIN) | GPIO23 |

## Build + flash
```bash
arduino-cli lib install "GxEPD2"
arduino-cli compile --fqbn esp32:esp32:esp32 .
espflash flash --port /dev/ttyUSB0 --baud 460800 \
    build/esp32.esp32.esp32/labwired-ereader.ino.elf
```

## Confirmed timing on real hardware (esp32 v1.0, C90c panel)
- `_PowerOn`: ~95 ms
- `_Update_Full`: ~18.5 s (full tri-color refresh)
- `_PowerOff`: ~141 ms

## Gotcha: driver-class selection
The 2.9" tri-color Waveshare panels look identical externally but use
different driver ICs. **C90c** is the one on the AgentDeck hardware
(verified against /home/andrii/Projects/AgentDeck/firmware libdeps).
Picking `Z13c` instead makes the library report all refresh stages
"succeeded" in microseconds (BUSY pin returns instantly because the
wrong init left the panel in a no-op state) — panel goes blank.

## Sim status (as of 2026-05-23)
Real hardware: ✅ verified painting.
LabWired sim: ❌ stuck in newlib `__sflags` at 200M cycles, never
reaches `setup()`. Tracked as #115 — needs more thunk coverage for
the libc stdio init path the reader sketch triggers but AgentDeck did
not.
