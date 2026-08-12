// LabWired Arduino matrix L8 — on-chip CAN loopback (register-level).
//
// Stock Arduino has no portable CAN API. This sketch pokes bxCAN / FDCAN
// registers using the same sequences as engine unit tests. Avoid CMSIS names
// (CAN1, CAN_BASE, FDCAN1, RCC_BASE, …) — they are macros/types in STM headers.

#include <Arduino.h>
#include <stdint.h>

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

static inline void mmio_w(uint32_t addr, uint32_t val) {
  *(volatile uint32_t *)(uintptr_t)addr = val;
}
static inline uint32_t mmio_r(uint32_t addr) {
  return *(volatile uint32_t *)(uintptr_t)addr;
}

// Prefer the most specific board first; use #elif chain so only one path.

#if defined(ARDUINO_NUCLEO_H563ZI) || defined(STM32H563xx) || defined(STM32H563ZITx)
// H5 FDCAN1
static bool lw_can_probe() {
  const uint32_t fd = 0x4000A400u;
  const uint32_t rcc = 0x44020C00u;
  mmio_w(rcc + 0xE8, mmio_r(rcc + 0xE8) | (1u << 9));
  mmio_w(fd + 0x18, 0x3u);
  mmio_w(fd + 0x18, mmio_r(fd + 0x18) | (1u << 7));
  mmio_w(fd + 0x10, (1u << 4));
  mmio_w(fd + 0x1C, 0x06000A03u);
  mmio_w(fd + 0x18, (1u << 7));
  mmio_w(fd + 0x800 + 0x278, (0x123u << 18));
  mmio_w(fd + 0x800 + 0x27C, (1u << 16));
  mmio_w(fd + 0x800 + 0x280, 0xA5u);
  mmio_w(fd + 0x0CC, 1u);
  for (int i = 0; i < 10000; i++) {
    if ((mmio_r(fd + 0x90) & 0x7Fu) != 0) {
      uint32_t w0 = mmio_r(fd + 0x800 + 0xB0);
      uint32_t w2 = mmio_r(fd + 0x800 + 0xB0 + 8);
      return ((w0 >> 18) & 0x7FFu) == 0x123u && (w2 & 0xFFu) == 0xA5u;
    }
  }
  return false;
}
#define LW_HAS_CAN 1

#elif defined(ARDUINO_NUCLEO_L476RG) || defined(STM32L476xx)
// L4 bxCAN1
static bool lw_can_probe() {
  const uint32_t can = 0x40006400u;
  const uint32_t rcc = 0x40021000u;
  mmio_w(rcc + 0x58, mmio_r(rcc + 0x58) | (1u << 25));
  mmio_w(can + 0x00, 0x1);
  mmio_w(can + 0x1C, 0x405C0009u | (1u << 30));
  mmio_w(can + 0x200, 0x2A1C0E01u);
  mmio_w(can + 0x204, 0);
  mmio_w(can + 0x20C, 1);
  mmio_w(can + 0x214, 0);
  mmio_w(can + 0x240, 0);
  mmio_w(can + 0x244, 0);
  mmio_w(can + 0x21C, 1);
  mmio_w(can + 0x200, 0x2A1C0E00u);
  mmio_w(can + 0x00, 0);
  mmio_w(can + 0x184, (1u << 16) | 1u);
  mmio_w(can + 0x188, 0xA5u);
  mmio_w(can + 0x180, (0x123u << 21) | 1u);
  for (int i = 0; i < 10000; i++) {
    if ((mmio_r(can + 0x0C) & 0x3u) != 0) {
      uint32_t rir = mmio_r(can + 0x1B0);
      uint32_t rdl = mmio_r(can + 0x1B8);
      mmio_w(can + 0x0C, (1u << 5));
      return ((rir >> 21) & 0x7FFu) == 0x123u && (rdl & 0xFFu) == 0xA5u;
    }
  }
  return false;
}
#define LW_HAS_CAN 1

#elif defined(STM32F1xx) || defined(ARDUINO_BLUEPILL_F103C8) || defined(ARDUINO_GENERIC_F103C8TX) || \
    defined(ARDUINO_BLUEPILL_F103CB) || defined(STM32F103xB) || defined(STM32F103xE)
// F1 bxCAN1
static bool lw_can_probe() {
  const uint32_t can = 0x40006400u;
  const uint32_t rcc = 0x40021000u;
  mmio_w(rcc + 0x1C, mmio_r(rcc + 0x1C) | (1u << 25));
  mmio_w(can + 0x00, 0x1);
  mmio_w(can + 0x1C, 0x405C0009u | (1u << 30));
  mmio_w(can + 0x200, 0x2A1C0E01u);
  mmio_w(can + 0x204, 0);
  mmio_w(can + 0x20C, 1);
  mmio_w(can + 0x214, 0);
  mmio_w(can + 0x240, 0);
  mmio_w(can + 0x244, 0);
  mmio_w(can + 0x21C, 1);
  mmio_w(can + 0x200, 0x2A1C0E00u);
  mmio_w(can + 0x00, 0);
  mmio_w(can + 0x184, (1u << 16) | 1u);
  mmio_w(can + 0x188, 0xA5u);
  mmio_w(can + 0x180, (0x123u << 21) | 1u);
  for (int i = 0; i < 10000; i++) {
    if ((mmio_r(can + 0x0C) & 0x3u) != 0) {
      uint32_t rir = mmio_r(can + 0x1B0);
      uint32_t rdl = mmio_r(can + 0x1B8);
      mmio_w(can + 0x0C, (1u << 5));
      return ((rir >> 21) & 0x7FFu) == 0x123u && (rdl & 0xFFu) == 0xA5u;
    }
  }
  return false;
}
#define LW_HAS_CAN 1

#else
static bool lw_can_probe() { return false; }
#define LW_HAS_CAN 0
#endif

void setup() {
  logBegin();
  logLine("LW_L8_BOOT");
#if LW_HAS_CAN
  if (lw_can_probe()) {
    logLine("LW_L8_OK");
  } else {
    logLine("LW_L8_FAIL");
  }
#else
  logLine("LW_L8_FAIL no_can");
#endif
}

void loop() {}
