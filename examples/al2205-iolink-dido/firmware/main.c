/* AL2205-style IO-Link DI device — firmware-under-test.
 *
 * M2: bring up the iolinki device stack over the USART2 PHY and run its loop
 * with a constant process-data input. The native IO-Link master (M3) and the
 * 74HC165 input shifter (M4) are wired in later milestones.
 */
#include "iolinki/iolink.h"
#include "iolinki/application.h"
#include "phy_labwired.h"
#include "debug_uart.h"
#include <string.h>

int main(void) {
    dbg_uart_init();
    dbg_puts("AL2205 BOOT\r\n");

    /* Zero the whole struct first: on this toolchain (arm-none-eabi GCC 10.2,
     * -Os, short-enums) a designated-initializer left t_pd_us uninitialised,
     * which made the stack arm a bogus power-on delay (t_pd) that never
     * expired. memset + explicit assignment is robust. */
    iolink_config_t cfg;
    memset(&cfg, 0, sizeof(cfg));
    cfg.m_seq_type = IOLINK_M_SEQ_TYPE_1_1;
    cfg.min_cycle_time = 0;
    cfg.pd_in_len = 1;
    cfg.pd_out_len = 0;
    cfg.t_pd_us = 0;
    if (iolink_init(iolink_phy_labwired_get(), &cfg) != 0) {
        dbg_puts("IOLINK INIT FAIL\r\n");
        for (;;) {
        }
    }
    iolink_set_timing_enforcement(false);
    dbg_puts("IOLINK INIT OK\r\n");

    uint8_t pd = 0x00;
    iolink_dll_state_t last = (iolink_dll_state_t)0xFF;
    for (;;) {
        iolink_pd_input_update(&pd, 1, true);
        iolink_process();
        /* Deliberately do NOT advance g_iolink_ticks_ms: the CPU loops far
         * faster than the simulated UART byte rate, so a per-loop tick would
         * race the stack's millisecond timeouts (e.g. the >1000 ms inactivity
         * watchdog resets the link to STARTUP). With the clock frozen and
         * timing enforcement off, the handshake is driven purely by byte
         * arrival, which is what the cycle-stepped simulator models. */

        iolink_dll_state_t s = iolink_get_state();
        if (s != last) {
            last = s;
            /* Trace transitions (so a stall is visible); flag OPERATE for the gate. */
            dbg_puts("STATE=");
            dbg_hex8((unsigned char)s);
            if (s == IOLINK_DLL_STATE_OPERATE) {
                dbg_puts(" OPERATE");
            }
            dbg_puts("\r\n");
        }
    }
}
