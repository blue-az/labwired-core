// PING PONG — Player A (the server)
// ---------------------------------
// Two ESP32-C3s rally a "ball" back and forth over a plain serial wire, with
// no host, no WiFi, and no coordinator. This board serves; the other board
// returns and draws the match on its OLED.
//
// Wiring (cross-connected, TX to RX both ways):
//   A GPIO6 (TX) ---> B GPIO7 (RX)
//   A GPIO7 (RX) <--- B GPIO6 (TX)
//   A GND        ---- B GND        <- shared ground, or neither side sees a byte
//
// The rally is self-sustaining: every PONG that comes back is immediately
// answered with the next PING, so the two boards keep each other running.

#include <Arduino.h>

static constexpr int PIN_LINK_TX = 6;
static constexpr int PIN_LINK_RX = 7;
static constexpr uint32_t LINK_BAUD = 115200;

// If a return never arrives the rally is dead; serve a fresh ball rather than
// waiting forever, so unplugging one board and plugging it back in recovers.
static constexpr uint32_t RETURN_TIMEOUT_MS = 1000;

static uint32_t rally = 0;      // consecutive successful returns
static uint32_t longest = 0;    // best rally this power-cycle
static uint32_t servedAt = 0;
static bool waitingForReturn = false;

static void serve() {
  Serial1.print("PING\n");
  servedAt = millis();
  waitingForReturn = true;
}

void setup() {
  Serial.begin(115200);
  // Serial1 is the inter-board link. Serial (UART0) stays the USB console, so
  // the rally traffic and the commentary never share a wire.
  Serial1.begin(LINK_BAUD, SERIAL_8N1, PIN_LINK_RX, PIN_LINK_TX);
  Serial.println("Player A ready - serving");
  serve();
}

void loop() {
  static char inbox[8];
  static uint8_t filled = 0;

  while (Serial1.available()) {
    char c = Serial1.read();
    if (c == '\n') {
      inbox[filled] = '\0';
      if (strcmp(inbox, "PONG") == 0) {
        rally++;
        if (rally > longest) longest = rally;
        Serial.print("rally ");
        Serial.println(rally);
        serve();  // returned - keep the ball moving
      }
      filled = 0;
    } else if (filled < sizeof(inbox) - 1) {
      inbox[filled++] = c;
    }
  }

  if (waitingForReturn && millis() - servedAt > RETURN_TIMEOUT_MS) {
    Serial.print("missed - rally ended at ");
    Serial.println(rally);
    rally = 0;
    serve();
  }
}
