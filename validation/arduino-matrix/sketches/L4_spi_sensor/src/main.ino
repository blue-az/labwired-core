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
  // Flush any residual RX after SPI.begin() (STM32 DR often holds a byte).
  digitalWrite(LW_SPI_CS, HIGH);
  (void)SPI.transfer(0x00);

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

  uint32_t frame = readMax31855();

  // Datasheet: bit16 fault. Default declarative kit: 0x01901600 (25°C / 22°C).
  bool fault = (frame & (1u << 16)) != 0;
  int16_t tc_raw = (int16_t)((frame >> 18) & 0x3FFF);
  if (tc_raw & 0x2000) {
    tc_raw |= (int16_t)0xC000;
  }

  // Accept the known default frame OR any fault-free in-range thermocouple word.
  if (frame == 0x01901600u || (!fault && tc_raw > -200 && tc_raw < 2000 && frame != 0)) {
    logLine("LW_L4_OK");
    return;
  }
  char buf[56];
  snprintf(buf, sizeof(buf), "LW_L4_FAIL frame=0x%08lx fault=%u raw=%d",
           (unsigned long)frame, fault ? 1u : 0u, (int)tc_raw);
  logLine(buf);
}

void loop() {}
