// LabWired Arduino matrix L5 — ADC via stock analogRead().
//
// Proves: Arduino ADC path completes a conversion (model must not hang in
// SWSTART/EOC poll). Accepts any in-range count; mid-scale is typical when
// the twin injects ~Vref/2 or leaves an unconnected channel at idle.

#include <Arduino.h>

#ifndef LW_ADC_PIN
#  if defined(A0)
#    define LW_ADC_PIN A0
#  else
#    define LW_ADC_PIN 0
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

void setup() {
  logBegin();
  logLine("LW_L5_BOOT");

  // Discard first sample (some cores warm the S&H / channel mux).
  (void)analogRead(LW_ADC_PIN);
  int v = analogRead(LW_ADC_PIN);

  // 10-bit (AVR classic) through 12-bit (STM32/nRF/ESP) full-scale.
  if (v >= 0 && v <= 4095) {
    logLine("LW_L5_OK");
    return;
  }
  char buf[40];
  snprintf(buf, sizeof(buf), "LW_L5_FAIL v=%d", v);
  logLine(buf);
}

void loop() {}
