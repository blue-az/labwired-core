# E-Paper Weather Station (ESP32-C3 + WeAct 2.9″ + GxEPD2 C90c)

A desk weather display you can actually reuse. An ESP32-C3 joins your WiFi,
fetches the current conditions for your city from Open-Meteo — free, no account,
no API key — and paints them on a 2.9″ black/white/red e-ink panel. It re-fetches
on a timer; between refreshes the panel draws no current at all, so the last
reading stays readable even with the board unplugged.

Set what's yours at the top of `src/main.ino`:

```c
static const char *WIFI_SSID = "labwired-ap";      // your WiFi
static const char *WIFI_PASS = "";                 // "" for an open network
static const char *TITLE = "ANDRII'S WEATHER";     // the red line across the top
static const char *CITY = "KYIV";                  // label + coordinates below
static const float LAT = 50.45f;
static const float LON = 30.52f;
static const unsigned long REFRESH_MINUTES = 30;
```

Find your coordinates with the city search at
[open-meteo.com/en/docs](https://open-meteo.com/en/docs).

`WIFI_SSID` ships as `labwired-ap` so the lab runs in the simulator out of the
box — change it to your home network before flashing real hardware.

## What lands on the panel

```
ANDRII'S WEATHER                      KYIV 21:00
────────────────────────────────────────────────
                     PARTLY CLOUDY
  23°                 HUMIDITY  50%
                     HI 25.7   LO 14.5
```

Title and temperature in red, everything else black. Conditions come from the
WMO weather code, mapped to short labels in `code_text()`.

## Refresh rate

Tri-color panels are slow: a full refresh takes ~15 s, and the panel datasheets
ask for at least ~3 minutes between updates. Keep `REFRESH_MINUTES` well above 5.
Upstream data only moves every 15 minutes anyway, so 30 is a good default.

The sketch stays awake on a `millis()` timer rather than deep sleep, which keeps
it simple and observable in the twin. For a battery build, replace the `loop()`
timer with `esp_deep_sleep_start()` after the paint.

## Correct lock (do not break)

| Layer | Value |
|--------|--------|
| **Driver** | `GxEPD2_290_C90c` |
| **Twin / diagram type** | **`ssd1680_tricolor_290`** |
| **Not** | `uc8151d_tricolor_290` (that is the `GxEPD2_290_Z13c` panel) |

Several WeAct panels this size look identical but need different driver opcodes,
so the twin has to match the *driver class* you instantiate — which is a property
of the class, not of the MCU.

Despite the name, `GxEPD2_290_C90c` speaks **SSD1680**. Read
`GxEPD2_290_C90c::_InitDisplay()` in GxEPD2 1.6.0: `0x12` SWRESET, `0x01` driver
output control, `0x11` data entry, `0x3C` border, `0x21`, then `0x22`+`0x20` to
trigger the update, with RAM writes on `0x24`/`0x26`. Captured off the wire, a
C90c build sends:

```
12 01 27 01 00 11 03 3c 05 18 80 21 00 80 44 ...   <- SSD1680
```

A `GxEPD2_290_Z13c` build — the UC8151D panel — sends something else entirely:

```
00 8f 61 80 01 28 50 77 04                          <- UC8151D (PSR, TRES, CDI, PON)
```

`ssd1680_tricolor_290` is written against exactly the 15 commands C90c emits.
Pick `uc8151d_tricolor_290` here and the panel decodes that stream as PWR/LUT/DRF,
never receives a plane, and stays blank — which is precisely what happened to the
labwired-ereader lab until 2026-07-30.

## Pins (weather-station diagram)

| Signal | GPIO |
|--------|------|
| SCK | 4 |
| MOSI | 6 |
| CS | 7 |
| DC | 2 |
| RST | 3 |
| BUSY | 5 |

## Buy / docs

- [WeAct 2.9″ B/W/R (AliExpress)](https://www.aliexpress.com/item/1005004644515880.html)
- [WeAct EpaperModule](https://github.com/WeActStudio/WeActStudio.EpaperModule)
- [Open-Meteo forecast API](https://open-meteo.com/en/docs)

## Build

Arduino-ESP32 + `zinggjm/GxEPD2`. The library is **not** inferred from the
`#include` — you have to declare it, or the build stops at
`GxEPD2_3C.h: No such file or directory`.

Hosted compile — pass it in `lib_deps`:

```json
{ "board": "esp32-c3-supermini", "language": "arduino",
  "entryPath": "src/main.ino", "lib_deps": ["zinggjm/GxEPD2"] }
```

Locally with PlatformIO:

```ini
[env:esp32c3]
platform = espressif32
board = esp32-c3-devkitm-1
framework = arduino
lib_deps = zinggjm/GxEPD2
```

That pulls Adafruit GFX and BusIO as dependencies and builds clean (~780 KB
flash, 60% of a 4 MB part). Or paste `src/main.ino` into the LabWired
weather-station project and flash.
