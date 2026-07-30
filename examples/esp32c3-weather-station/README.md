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
| **Twin / diagram type** | **`uc8151d_tricolor_290`** |
| **Not** | `ssd1680_tricolor_290` + raw `0x24`/`0x26` |

Several WeAct panels this size look identical but need different driver opcodes.
GxEPD2's C90c class speaks **UC8151D-style** commands. The SSD1680 twin treats
`0x12` as SWRESET, so the same sketch can paint on glass and stay blank in the
emulator if the diagram is locked to SSD1680. This was fixed for labwired-ereader
in core (2026-06-28); this lab must use the same lock.

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

Arduino-ESP32 + `zinggjm/GxEPD2` (inferred from `#include <GxEPD2_3C.h>` on hosted compile).
Or paste `src/main.ino` into the LabWired weather-station project and flash.
