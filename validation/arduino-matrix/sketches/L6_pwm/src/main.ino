// LabWired Arduino matrix L6 — PWM via stock analogWrite().
//
// Proves: Arduino PWM / timer-compare path accepts a duty write without
// hanging (LEDC / TIM OC / nRF PWM / RP2040 slice — whatever the core uses).

#include <Arduino.h>

#ifndef LW_PWM_PIN
  // Nucleo-G474RE LED is PA5, which is also DAC1_OUT2 — analogWrite prefers
  // DAC and the twin has no DAC window → memory_violation. Use TIM2_CH1 PA0.
#  if defined(ARDUINO_NUCLEO_G474RE) || defined(ARDUINO_NUCLEO_G474RE_P)
#    define LW_PWM_PIN PA0
#  elif defined(LED_BUILTIN)
#    define LW_PWM_PIN LED_BUILTIN
#  elif defined(ARDUINO_ARCH_ESP32)
#    define LW_PWM_PIN 2
#  elif defined(ARDUINO_ARCH_RP2040)
#    define LW_PWM_PIN 25
#  else
#    define LW_PWM_PIN 9
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
  logLine("LW_L6_BOOT");

  pinMode(LW_PWM_PIN, OUTPUT);
  // Mid duty then off — exercises timer OCR / LEDC duty latch.
  analogWrite(LW_PWM_PIN, 128);
  delay(1);
  analogWrite(LW_PWM_PIN, 0);
  delay(1);

  logLine("LW_L6_OK");
}

void loop() {}
