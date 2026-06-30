/* PHY backend over a simulated STM32L476 USART (stm32v2 register layout).
 *
 * The simulator transmits on any TDR write and reports TXE ready, and exposes
 * received bytes via RXNE/RDR, so only a token CR1 (UE|TE|RE) init is needed.
 * The IO-Link line speed is irrelevant in the cycle-stepped sim, so set_baudrate
 * is a no-op. detect_wakeup scans for the 0x55 wake-up byte (mirrors phy_virtual).
 */
#include "phy_labwired.h"
#include <stdint.h>

#define REG(a) (*(volatile uint32_t *)(a))
#define ISR_RXNE (1u << 5)
#define ISR_TXE (1u << 7)
#define CR1_UE (1u << 0)
#define CR1_RE (1u << 2)
#define CR1_TE (1u << 3)

static uintptr_t base_from_user(void *user) {
    return (uintptr_t)user;
}

static int phy_init(void *user) {
    REG(base_from_user(user) + 0x00u) = CR1_UE | CR1_TE | CR1_RE;
    return 0;
}

static void phy_set_mode(void *user, iolink_phy_mode_t mode) {
    (void)user;
    (void)mode;
}

static void phy_set_baudrate(void *user, iolink_baudrate_t baudrate) {
    (void)user;
    (void)baudrate;
}

static int phy_send(void *user, const uint8_t *data, size_t len) {
    uintptr_t base = base_from_user(user);
    for (size_t i = 0; i < len; i++) {
        while ((REG(base + 0x1Cu) & ISR_TXE) == 0u) {
        }
        REG(base + 0x28u) = (uint32_t)data[i];
    }
    return (int)len;
}

static int phy_recv_byte(void *user, uint8_t *byte) {
    uintptr_t base = base_from_user(user);
    if (REG(base + 0x1Cu) & ISR_RXNE) {
        *byte = (uint8_t)REG(base + 0x24u);
        return 1;
    }
    return 0;
}

static int phy_detect_wakeup(void *user) {
    uint8_t b;
    while (phy_recv_byte(user, &b) > 0) {
        if (b == 0x55u) {
            return 1;
        }
    }
    return 0;
}

void iolink_phy_labwired_init(iolink_phy_api_t *phy, uintptr_t usart_base) {
    phy->user = (void *)usart_base;
    phy->init = phy_init;
    phy->set_mode = phy_set_mode;
    phy->set_baudrate = phy_set_baudrate;
    phy->send = phy_send;
    phy->recv_byte = phy_recv_byte;
    phy->detect_wakeup = phy_detect_wakeup;
    phy->set_cq_line = 0;
    phy->get_voltage_mv = 0;
    phy->is_short_circuit = 0;
}
