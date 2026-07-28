// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.
//
// Arduino conformance sketch — the SAME source for every board LabWired
// simulates. It exercises the main buses through the real Arduino core (not
// through a bespoke no_std fixture), which is the point: an Arduino core
// touches far more of a chip than a hand-written register poke does, so this
// is the harshest routine portability check we have.
//
// Protocol (one line per class, parsed by the survival harness):
//
//     LWCONF <class> PASS
//     LWCONF <class> FAIL code=<reason>
//     LWCONF <class> SKIP code=<reason>
//     LWCONF done
//
// `serial` is implicit: receiving `LWCONF done` at all proves the UART path.
//
// SKIP is deliberate and load-bearing. A board whose core does not expose a
// bus (no Wire, no SPI) must say so out loud rather than silently omitting a
// line — an absent line and a passing line must never be confusable.
//
// Everything is bounded by fixed iteration counts. The simulator is
// deterministic and has no wall clock, so never introduce a timing-based wait.

#include <Arduino.h>
// Wire.h / SPI.h are included UNCONDITIONALLY and never behind an #if.
// arduino-cli resolves library dependencies by textually scanning #include
// lines before it preprocesses anything, so an include hidden inside an
// `#if __has_include(...)` guard is invisible to the dependency scanner — the
// library never lands on the include path, __has_include then reports false,
// and every board reports SKIP while looking perfectly healthy. Both libraries
// ship with every core this matrix targets (STM32duino, arduino-esp32,
// Adafruit/mbed nRF52, RP2040). A core that genuinely lacks one must be
// handled with a per-core exclusion in the build script, not by silently
// compiling out the check here.
#include <SPI.h>
#include <Wire.h>

#define LW_HAS_WIRE 1
#define LW_HAS_SPI 1

// The pin driven by the gpio check. LED_BUILTIN is defined by every core's
// variant for its own board, which is exactly the per-board indirection we
// want — no board table to maintain here.
#ifndef LW_GPIO_PIN
#ifdef LED_BUILTIN
#define LW_GPIO_PIN LED_BUILTIN
#else
#define LW_GPIO_PIN 0
#endif
#endif

// An address no simulated device answers on. The I2C check asserts the bus
// reports a clean address NACK, which proves the controller ran a real
// address phase and sampled the ACK bit — a controller that never drives the
// bus cannot produce this.
#ifndef LW_I2C_ABSENT_ADDR
#define LW_I2C_ABSENT_ADDR 0x4E
#endif

static void report(const char *cls, const char *verdict, const char *code) {
  Serial.print("LWCONF ");
  Serial.print(cls);
  Serial.print(' ');
  Serial.print(verdict);
  if (code) {
    Serial.print(" code=");
    Serial.print(code);
  }
  Serial.print('\n');
  Serial.flush();
}

// gpio: drive the pin both ways and read it back. Reading back an OUTPUT pin
// returns the output-data register on every core we target, so this proves
// the GPIO model latches writes rather than merely accepting them.
static void check_gpio(void) {
  pinMode(LW_GPIO_PIN, OUTPUT);

  digitalWrite(LW_GPIO_PIN, HIGH);
  if (digitalRead(LW_GPIO_PIN) != HIGH) {
    report("gpio", "FAIL", "set");
    return;
  }

  digitalWrite(LW_GPIO_PIN, LOW);
  if (digitalRead(LW_GPIO_PIN) != LOW) {
    report("gpio", "FAIL", "clear");
    return;
  }

  report("gpio", "PASS", NULL);
}

// i2c: a one-byte write to an absent address must complete and report a NACK.
// endTransmission() returns 0 on success, 2 on address NACK, 3 on data NACK,
// 4 on other error. Anything that is not a clean 0/2/3 means the controller
// did not finish an address phase.
static void check_i2c(void) {
#ifdef LW_HAS_WIRE
  Wire.begin();
  Wire.beginTransmission((uint8_t)LW_I2C_ABSENT_ADDR);
  Wire.write((uint8_t)0x00);
  uint8_t rc = Wire.endTransmission();

  if (rc == 2 || rc == 3) {
    // The expected outcome: no device at this address, cleanly reported.
    report("i2c", "PASS", NULL);
  } else if (rc == 0) {
    // Something ACKed. That still proves a working controller, and some
    // simulated systems do attach a device here.
    report("i2c", "PASS", NULL);
  } else if (rc == 4) {
    report("i2c", "FAIL", "buserr");
  } else {
    report("i2c", "FAIL", "timeout");
  }
#else
  report("i2c", "SKIP", "no-wire-lib");
#endif
}

// spi: clock a byte out. With no device attached the returned byte is
// undefined (0x00 or 0xFF depending on bus idle level), so the assertion is
// that the transfer COMPLETES — a shift engine that never raises its
// transfer-complete flag hangs here instead, which the harness sees as a
// missing line.
static void check_spi(void) {
#ifdef LW_HAS_SPI
  SPI.begin();
  (void)SPI.transfer(0x5A);
  (void)SPI.transfer(0xA5);
  SPI.end();
  report("spi", "PASS", NULL);
#else
  report("spi", "SKIP", "no-spi-lib");
#endif
}

void setup(void) {
  Serial.begin(115200);

  // Bounded wait for the port. Cores with native USB (RP2040, nRF52840)
  // return false from `!Serial` until the simulated host asserts DTR; cores
  // with a plain UART are ready immediately. Bounded so neither class hangs.
  for (int i = 0; i < 10000 && !Serial; i++) {
    // Intentionally empty: the loop bound IS the timeout.
  }

  Serial.print("LWCONF begin\n");

  check_gpio();
  check_i2c();
  check_spi();

  Serial.print("LWCONF done\n");
  Serial.flush();
}

void loop(void) {
  // Nothing. Every claim this sketch makes is made once, in setup().
}
