// E-Paper Weather Station — ESP32-C3 + WeAct 2.9" tri-color e-paper
//
// Joins your WiFi, pulls the current conditions for your city from Open-Meteo
// (free, no account, no API key), paints them on the panel, and repeats every
// REFRESH_MINUTES. Between refreshes the display draws no current at all — the
// last reading stays on the glass even if you unplug the board.
//
// Everything you need to change is in the EDIT ME block below.
//
// Uses the SAME proven display stack as labwired-ereader (works on glass + twin):
//   GxEPD2_290_C90c  +  diagram part type ssd1680_tricolor_290
//
// Despite the name, GxEPD2_290_C90c speaks SSD1680: 0x12 SWRESET, 0x11 data
// entry, 0x24/0x26 RAM writes, 0x22+0x20 to refresh. Lock the twin to
// uc8151d_tricolor_290 and it decodes those as PWR/LUT/DRF, never receives a
// plane, and the panel stays blank while the same sketch paints fine on glass.
//
// Panel (buy): WeAct Studio 2.9" B/W/R
//   https://www.aliexpress.com/item/1005004644515880.html
// Module docs: https://github.com/WeActStudio/WeActStudio.EpaperModule
//
// Pins match the LabWired weather-station diagram (ESP32-C3 SuperMini):
//   SCK=4  MOSI=6  CS=7  DC=2  RST=3  BUSY=5
//
// Flow: paint boot -> join WiFi -> GET api.open-meteo.com -> paint weather
// (or offline card), then re-fetch on a timer. Serial marker "PANEL UPDATED"
// after each full refresh.
#include <WiFi.h>
#include <SPI.h>
#include <GxEPD2_3C.h>
#include <Fonts/FreeSansBold9pt7b.h>
#include <Fonts/FreeSans9pt7b.h>
#include <Fonts/FreeSansBold12pt7b.h>
#include <Fonts/FreeSansBold24pt7b.h>

// ===================== EDIT ME =====================

// Your WiFi. Leave WIFI_PASS as "" for an open network.
// "labwired-ap" is the simulator's access point — change it to your home WiFi
// before flashing real hardware.
static const char *WIFI_SSID = "labwired-ap";
static const char *WIFI_PASS = "";

// Shown in red across the top. Make it yours.
static const char *TITLE = "ANDRII'S WEATHER";

// Your city: the label to print, and its coordinates.
// Look coordinates up at https://open-meteo.com/en/docs (search your city).
static const char *CITY = "KYIV";
static const float LAT = 50.45f;
static const float LON = 30.52f;

// How often to re-fetch and repaint.
// Tri-color panels take ~15 s per full refresh and their datasheets ask for at
// least ~3 minutes between updates, so keep this comfortably above 5.
static const unsigned long REFRESH_MINUTES = 30;

// =================== END EDIT ME ===================

// ---- Pins (diagram: mcu -> ep) ----
static const int PIN_SCK = 4;
static const int PIN_MOSI = 6;
static const int PIN_CS = 7;
static const int PIN_DC = 2;
static const int PIN_RST = 3;
static const int PIN_BUSY = 5;

static const char *API_HOST = "api.open-meteo.com";

// C90c class = SSD1680 command stream on the wire -> twin must be ssd1680_tricolor_290
GxEPD2_3C<GxEPD2_290_C90c, GxEPD2_290_C90c::HEIGHT> display(
    GxEPD2_290_C90c(/*CS=*/PIN_CS, /*DC=*/PIN_DC, /*RST=*/PIN_RST, /*BUSY=*/PIN_BUSY));

struct Weather {
  float temp = NAN;
  float hi = NAN;
  float lo = NAN;
  int humidity = -1;
  int code = -1;
  String clock_hhmm;  // local time of the reading, "21:00"
};

static void panel_updated(const char *why) {
  Serial.print("PANEL UPDATED");
  if (why && why[0]) {
    Serial.print(" (");
    Serial.print(why);
    Serial.print(")");
  }
  Serial.println();
}

// WMO weather interpretation codes -> short label.
// https://open-meteo.com/en/docs (see "Weather variable documentation")
static const char *code_text(int code) {
  switch (code) {
    case 0: return "CLEAR";
    case 1: return "MAINLY CLEAR";
    case 2: return "PARTLY CLOUDY";
    case 3: return "OVERCAST";
    case 45: case 48: return "FOG";
    case 51: case 53: case 55: return "DRIZZLE";
    case 56: case 57: return "FREEZING DRIZZLE";
    case 61: case 63: case 65: return "RAIN";
    case 66: case 67: return "FREEZING RAIN";
    case 71: case 73: case 75: return "SNOW";
    case 77: return "SNOW GRAINS";
    case 80: case 81: case 82: return "SHOWERS";
    case 85: case 86: return "SNOW SHOWERS";
    case 95: return "THUNDERSTORM";
    case 96: case 99: return "THUNDER + HAIL";
    default: return "";
  }
}

// The Free* fonts only carry ASCII 0x20-0x7E, so there is no degree glyph.
// Draw the ring by hand, then the unit letter.
static void print_degree_c(int16_t x, int16_t y, uint8_t radius, uint16_t color) {
  display.drawCircle(x + radius, y + radius, radius, color);
  display.setCursor(x + 2 * radius + 3, y + 2 * radius + 2);
  display.print("C");
}

static void draw_header() {
  display.setTextColor(GxEPD_RED);
  display.setFont(&FreeSansBold9pt7b);
  display.setCursor(6, 18);
  display.print(TITLE);
  display.drawFastHLine(6, 24, 284, GxEPD_RED);
}

static void draw_boot(const char *status) {
  display.setFullWindow();
  display.firstPage();
  do {
    display.fillScreen(GxEPD_WHITE);
    draw_header();
    display.setTextColor(GxEPD_BLACK);
    display.setFont(&FreeSans9pt7b);
    display.setCursor(6, 48);
    display.print("E-PAPER . ESP32-C3");
    display.setCursor(6, 72);
    display.print(status ? status : "...");
  } while (display.nextPage());
  panel_updated(status);
}

static void draw_offline(const char *why) {
  display.setFullWindow();
  display.firstPage();
  do {
    display.fillScreen(GxEPD_WHITE);
    draw_header();
    display.setFont(&FreeSansBold12pt7b);
    display.setTextColor(GxEPD_RED);
    display.setCursor(6, 60);
    display.print("OFFLINE");
    display.setTextColor(GxEPD_BLACK);
    display.setFont(&FreeSans9pt7b);
    display.setCursor(6, 88);
    display.print(why ? why : "NO LINK");
  } while (display.nextPage());
  panel_updated("offline");
}

static void draw_weather(const Weather &w) {
  char line[48];
  display.setFullWindow();
  display.firstPage();
  do {
    display.fillScreen(GxEPD_WHITE);
    draw_header();

    // City, and the local time the reading is stamped with, top right.
    display.setTextColor(GxEPD_BLACK);
    display.setFont(&FreeSans9pt7b);
    snprintf(line, sizeof line, "%s %s", CITY, w.clock_hhmm.c_str());
    int16_t bx, by;
    uint16_t bw, bh;
    display.getTextBounds(line, 0, 0, &bx, &by, &bw, &bh);
    display.setCursor(290 - bw, 18);
    display.print(line);

    // Big current temperature, left half.
    display.setTextColor(GxEPD_RED);
    display.setFont(&FreeSansBold24pt7b);
    snprintf(line, sizeof line, "%d", (int)lroundf(w.temp));
    display.setCursor(6, 84);
    display.print(line);
    display.getTextBounds(line, 6, 84, &bx, &by, &bw, &bh);
    display.setFont(&FreeSansBold12pt7b);
    print_degree_c(6 + bw + 6, 50, 4, GxEPD_RED);

    // Conditions, right half.
    display.setTextColor(GxEPD_BLACK);
    display.setFont(&FreeSansBold9pt7b);
    display.setCursor(150, 52);
    display.print(code_text(w.code));

    display.setFont(&FreeSans9pt7b);
    if (w.humidity >= 0) {
      snprintf(line, sizeof line, "HUMIDITY  %d%%", w.humidity);
      display.setCursor(150, 76);
      display.print(line);
    }
    if (!isnan(w.hi) && !isnan(w.lo)) {
      snprintf(line, sizeof line, "HI %.1f   LO %.1f", w.hi, w.lo);
      display.setCursor(150, 100);
      display.print(line);
    }
  } while (display.nextPage());
  panel_updated("weather");
}

// --- Tiny JSON scraping -------------------------------------------------
// Open-Meteo repeats every key once under "*_units" (as a string) before the
// real numeric value, so every lookup must start at the section marker.

static int section_start(const String &b, const char *section) {
  return b.indexOf(String("\"") + section + "\":{");
}

// Value of `key` after `from`. Skips a leading '[' so daily arrays work too.
static const char *value_at(const String &b, const char *key, int from, int *found) {
  *found = -1;
  if (from < 0) return nullptr;
  String needle = String("\"") + key + "\":";
  int i = b.indexOf(needle, from);
  if (i < 0) return nullptr;
  i += needle.length();
  if (i < (int)b.length() && b[i] == '[') i++;
  *found = i;
  return b.c_str() + i;
}

static float json_float(const String &b, const char *key, int from) {
  int at;
  const char *p = value_at(b, key, from, &at);
  return p ? atof(p) : NAN;
}

static int json_int(const String &b, const char *key, int from) {
  int at;
  const char *p = value_at(b, key, from, &at);
  return p ? atoi(p) : -1;
}

// "2026-07-30T21:00" -> "21:00"
static String json_time_hhmm(const String &b, int from) {
  int at;
  if (!value_at(b, "time", from, &at)) return String("");
  int q1 = b.indexOf('"', at);
  if (q1 < 0) return String("");
  int q2 = b.indexOf('"', q1 + 1);
  if (q2 < 0) return String("");
  String t = b.substring(q1 + 1, q2);
  int tpos = t.indexOf('T');
  return (tpos >= 0) ? t.substring(tpos + 1) : t;
}

static bool http_get_forecast(String &out) {
  WiFiClient c;
  if (!c.connect(API_HOST, 80)) {
    Serial.println("HTTP connect() failed");
    return false;
  }
  String path = String("/v1/forecast?latitude=") + String(LAT, 4) +
                "&longitude=" + String(LON, 4) +
                "&current=temperature_2m,relative_humidity_2m,weather_code"
                "&daily=temperature_2m_max,temperature_2m_min"
                "&timezone=auto&forecast_days=1";
  c.print(String("GET ") + path + " HTTP/1.1\r\nHost: " + API_HOST +
          "\r\nConnection: close\r\n\r\n");

  String resp;
  unsigned long dl = millis() + 10000;
  while (millis() < dl && (c.connected() || c.available())) {
    while (c.available()) resp += (char)c.read();
    delay(10);
  }
  c.stop();
  int s = resp.indexOf("\r\n\r\n");
  out = (s >= 0) ? resp.substring(s + 4) : resp;
  Serial.print("HTTP BODY: ");
  Serial.println(out);
  return out.indexOf("\"current\":") >= 0;
}

static bool connect_wifi() {
  if (WiFi.status() == WL_CONNECTED) return true;
  WiFi.mode(WIFI_STA);
  if (WIFI_PASS && WIFI_PASS[0]) {
    WiFi.begin(WIFI_SSID, WIFI_PASS);
  } else {
    WiFi.begin(WIFI_SSID);
  }
  Serial.print("connecting to ");
  Serial.println(WIFI_SSID);
  unsigned long dl = millis() + 30000;
  while (WiFi.status() != WL_CONNECTED && millis() < dl) delay(200);
  if (WiFi.status() != WL_CONNECTED) {
    Serial.println("WiFi connect timeout");
    return false;
  }
  Serial.print("STA CONNECTED, GOT IP ");
  Serial.println(WiFi.localIP());
  return true;
}

static void refresh() {
  if (!connect_wifi()) {
    draw_offline("NO WIFI - CHECK SSID");
    return;
  }

  String body;
  if (!http_get_forecast(body)) {
    Serial.println("forecast fetch failed");
    draw_offline("FORECAST FETCH FAILED");
    return;
  }

  int cur = section_start(body, "current");
  int day = section_start(body, "daily");

  Weather w;
  w.temp = json_float(body, "temperature_2m", cur);
  w.humidity = json_int(body, "relative_humidity_2m", cur);
  w.code = json_int(body, "weather_code", cur);
  w.hi = json_float(body, "temperature_2m_max", day);
  w.lo = json_float(body, "temperature_2m_min", day);
  w.clock_hhmm = json_time_hhmm(body, cur);

  Serial.printf("PARSED temp=%.1f rh=%d code=%d hi=%.1f lo=%.1f at=%s\n",
                w.temp, w.humidity, w.code, w.hi, w.lo, w.clock_hhmm.c_str());

  if (isnan(w.temp)) {
    draw_offline("BAD FORECAST DATA");
    return;
  }
  draw_weather(w);
}

void setup() {
  Serial.begin(115200);
  delay(100);
  Serial.println("E-Paper Weather Station boot (GxEPD2_290_C90c / UC8151D twin)");

  // ESP32-C3: bind SPI to the diagram pins before GxEPD2 init.
  SPI.begin(PIN_SCK, /*MISO*/ -1, PIN_MOSI, PIN_CS);

  Serial.printf("pins: SCK=%d MOSI=%d CS=%d DC=%d RST=%d BUSY=%d\n",
                PIN_SCK, PIN_MOSI, PIN_CS, PIN_DC, PIN_RST, PIN_BUSY);
  pinMode(PIN_BUSY, INPUT);
  Serial.print("BUSY initial: ");
  Serial.println(digitalRead(PIN_BUSY) ? "HIGH" : "LOW");

  display.init(115200);
  display.setRotation(1);  // landscape 296x128 — same as working e-reader

  draw_boot("CONNECTING WIFI");
  refresh();
}

void loop() {
  static unsigned long last = millis();
  const unsigned long period = REFRESH_MINUTES * 60UL * 1000UL;
  // Unsigned subtraction, so this survives the millis() rollover.
  if (millis() - last >= period) {
    last = millis();
    refresh();
  }
  delay(1000);
}
