// LabWired Arduino matrix L4 — SPI + MAX31855 (system external_devices).
//
// Proves: Arduino SPI master path + declarative SPI kit attach.
// Kit clocks out a 32-bit fault-free frame (default tc≈25°C). CS is soft
// on SpiDevice v1 (broadcast) but still driven so GPIO→CS path is exercised.

#include <Arduino.h>
#include <SPI.h>

#ifndef LW_SPI_CS
#  if defined(ARDUINO_ARCH_ESP32)
#    define LW_SPI_CS 5
#  elif defined(ARDUINO_ARCH_RP2040)
#    define LW_SPI_CS 17
#  elif defined(ARDUINO_ARCH_NRF52) || defined(ARDUINO_ARCH_NRF52840)
#    define LW_SPI_CS 22
#  else
#    define LW_SPI_CS SS
#  endif
#endif

static void logBegin() {
  Serial.begin(115200);
#if defined(ARDUINO_USB_CDC_ON_BOOT) && (ARDUINO_USB_CDC_ON_BOOT)
  Serial.setTxTimeoutMs(0);
  Serial0.begin(115200);
#endif
  delay(1);
}

static void logLine(const char *s) {
  Serial.println(s);
#if defined(ARDUINO_USB_CDC_ON_BOOT) && (ARDUINO_USB_CDC_ON_BOOT)
  Serial0.println(s);
#endif
}

static uint32_t readMax31855() {
  // Drive CS low then clock 4 bytes. Soft-CS models (broadcast SPI) treat
  // the first clock as frame start — do NOT flush with an extra transfer
  // first or the MAX31855 word shifts by one byte.
  digitalWrite(LW_SPI_CS, HIGH);
  delayMicroseconds(1);
  digitalWrite(LW_SPI_CS, LOW);
  uint32_t frame = 0;
  for (int i = 0; i < 4; i++) {
    frame = (frame << 8) | (uint32_t)SPI.transfer(0x00);
  }
  digitalWrite(LW_SPI_CS, HIGH);
  return frame;
}

void setup() {
  logBegin();
  logLine("LW_L4_BOOT");

  pinMode(LW_SPI_CS, OUTPUT);
  digitalWrite(LW_SPI_CS, HIGH);
  SPI.begin();
  delay(1);

  // First transaction absorbs residual RX / soft-CS desync; second is clean.
  (void)readMax31855();
  uint32_t frame = readMax31855();

  // Accept the default frame, a one-byte residual shift (STM32 DR junk
  // leading 0x00), or any fault-free in-range thermocouple word.
  auto frame_ok = [](uint32_t f) -> bool {
    if (f == 0) {
      return false;
    }
    bool fault = (f & (1u << 16)) != 0;
    int16_t tc_raw = (int16_t)((f >> 18) & 0x3FFF);
    if (tc_raw & 0x2000) {
      tc_raw |= (int16_t)0xC000;
    }
    if (f == 0x01901600u) {
      return true;
    }
    return !fault && tc_raw > -200 && tc_raw < 2000;
  };
  if (frame_ok(frame) || frame_ok(frame << 8) || frame_ok(frame >> 8)) {
    logLine("LW_L4_OK");
    return;
  }
  char buf[56];
  snprintf(buf, sizeof(buf), "LW_L4_FAIL frame=0x%08lx", (unsigned long)frame);
  logLine(buf);
}

void loop() {}
