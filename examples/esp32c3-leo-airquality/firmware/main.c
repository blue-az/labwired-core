/* Leo air-quality sensor firmware — ESP32-C3 (RISC-V rv32imc).
 *
 * Boots on a simulated ESP32-C3, reads four air-quality sensors over the REAL
 * C3 I²C0 controller, and turns the raw measurements into a plain-language
 * verdict over UART0 — exactly Leo's product promise ("translates air data into
 * plain language"), running with no hardware in the loop.
 *
 * Three of the four drivers are the UNMODIFIED Sensirion embedded-i2c vendor
 * libraries running ON-TARGET (riscv32):
 *   - SCD41  CO₂ + temperature + humidity   (third_party/embedded-i2c-scd4x)
 *   - SGP41  VOC raw → VOC Index             (third_party/embedded-i2c-sgp41 +
 *                                             gas-index-algorithm)
 *   - SPS30  particulate matter (PM2.5 …)    (third_party/embedded-i2c-sps30)
 * The VEML7700 ambient-light driver is a small register-level driver (Vishay
 * ships no bare-metal C library). All four talk to the sensor device models
 * through genuine I²C transactions executed by the simulated C3 controller.
 *
 * The headline story is a room filling up: CO₂ climbs from fresh toward stuffy
 * and the verdict flips from "air quality is good" to "CO₂ climbing, crack a
 * window" — live, deterministic, reproducible.
 */
#include <stdbool.h>
#include <stdint.h>

#include "c3_uart.h"
#include "scd4x_i2c.h"
#include "sensirion_gas_index_algorithm.h"
#include "sensirion_i2c_hal.h"
#include "sgp41_i2c.h"
#include "sps30_i2c.h"
#include "veml7700_c3.h"

#define SCD41_ADDR 0x62
#define SPS30_ADDR 0x69

/* Number of measurement cycles to run before the demo halts. The CO₂ ramp
 * (start 450 → target 1400 ppm, alpha 0.08) crosses 1000 ppm by ~cycle 11, so
 * 64 cycles comfortably show the full fresh → stuffy transition. */
#define SAMPLES 64

/* Default RH/T compensation inputs for the SGP41 raw command, in the sensor's
 * tick encoding (50 %RH, 25 °C) per the datasheet — what a real integration
 * passes before the on-board humidity reading is wired in. */
#define SGP41_DEFAULT_RH 0x8000
#define SGP41_DEFAULT_T 0x6666

/* Emit the plain-language air verdict. CO₂ is the headline; particulates add a
 * secondary clause when they matter. Thresholds are firmware policy, blind to
 * the simulator's scene. */
static void print_verdict(uint16_t co2, uint16_t pm2_5) {
    uart_puts("AIR: ");
    if (co2 >= 1400) {
        uart_puts("stale air - ventilate now, CO2 is high");
    } else if (co2 >= 1000) {
        uart_puts("getting stuffy - CO2 climbing, crack a window");
    } else if (co2 >= 800) {
        uart_puts("okay - air is fine but CO2 is creeping up");
    } else {
        uart_puts("fresh - air quality is good");
    }
    if (pm2_5 >= 35) {
        uart_puts("; particulates unhealthy");
    } else if (pm2_5 >= 12) {
        uart_puts("; some haze in the air");
    }
    uart_puts("\r\n");
}

int main(void) {
    sensirion_i2c_hal_init();
    uart_puts("LEO BOOT\r\n");
    uart_puts("Leo air-quality sensor: ESP32-C3 + SCD41/SGP41/SPS30 + VEML7700\r\n");

    /* ── SCD41: CO₂ + T + RH ──────────────────────────────────────────────── */
    scd4x_init(SCD41_ADDR);
    scd4x_wake_up();
    scd4x_stop_periodic_measurement();
    scd4x_reinit();
    scd4x_start_periodic_measurement();
    uart_puts("SCD41 READY\r\n");

    /* ── SGP41: VOC/NOx raw + Sensirion Gas Index Algorithm ───────────────── */
    uint16_t sraw_voc_cond = 0;
    sgp41_execute_conditioning(SGP41_DEFAULT_RH, SGP41_DEFAULT_T, &sraw_voc_cond);
    uart_puts("SGP41 READY\r\n");

    /* ── SPS30: particulate matter (integer/uint16 output, 30-byte frame) ──── */
    sps30_init(SPS30_ADDR);
    sps30_wake_up();
    sps30_start_measurement(
        (sps30_output_format)SPS30_OUTPUT_FORMAT_OUTPUT_FORMAT_UINT16);
    uart_puts("SPS30 READY\r\n");

    /* ── VEML7700: ambient light ─────────────────────────────────────────── */
    veml7700_init();
    uart_puts("VEML7700 READY\r\n");

    GasIndexAlgorithmParams voc_params;
    GasIndexAlgorithm_init(&voc_params, GasIndexAlgorithm_ALGORITHM_TYPE_VOC);

    for (int cycle = 0; cycle < SAMPLES; cycle++) {
        /* SCD41 — CO₂ ppm, temperature (m°C), humidity (m%RH). */
        bool ready = false;
        uint16_t co2 = 0;
        int32_t temp_m_deg_c = 0;
        int32_t rh_m_pct = 0;
        scd4x_get_data_ready_status(&ready);
        scd4x_read_measurement(&co2, &temp_m_deg_c, &rh_m_pct);

        /* SGP41 — raw VOC ticks → VOC Index via the real gas-index algorithm. */
        uint16_t sraw_voc = 0;
        uint16_t sraw_nox = 0;
        sgp41_measure_raw_signals(SGP41_DEFAULT_RH, SGP41_DEFAULT_T, &sraw_voc,
                                  &sraw_nox);
        int32_t voc_index = 0;
        GasIndexAlgorithm_process(&voc_params, (int32_t)sraw_voc, &voc_index);

        /* SPS30 — particulate mass/number concentrations (integer mode). */
        uint16_t mc_1p0 = 0, mc_2p5 = 0, mc_4p0 = 0, mc_10p0 = 0;
        uint16_t nc_0p5 = 0, nc_1p0 = 0, nc_2p5 = 0, nc_4p0 = 0, nc_10p0 = 0;
        uint16_t typ_size = 0;
        uint16_t pm_flag = 0;
        sps30_read_data_ready_flag(&pm_flag);
        sps30_read_measurement_values_uint16(&mc_1p0, &mc_2p5, &mc_4p0, &mc_10p0,
                                             &nc_0p5, &nc_1p0, &nc_2p5, &nc_4p0,
                                             &nc_10p0, &typ_size);

        /* VEML7700 — ambient light in lux. */
        uint16_t als_counts = 0;
        veml7700_read_als(&als_counts);
        uint32_t lux = veml7700_counts_to_lux(als_counts);

        /* Per-cycle measurement line. */
        uart_puts("t=");
        uart_puti(cycle);
        uart_puts(" CO2=");
        uart_puti((int32_t)co2);
        uart_puts("ppm T=");
        uart_putfix2(temp_m_deg_c / 10); /* m°C → value×100 */
        uart_puts("C RH=");
        uart_puti(rh_m_pct / 1000); /* m%RH → % */
        uart_puts("% PM2.5=");
        uart_puti((int32_t)mc_2p5);
        uart_puts("ug VOC=");
        uart_puti(voc_index);
        uart_puts(" LUX=");
        uart_puti((int32_t)lux);
        uart_puts("\r\n");

        print_verdict(co2, mc_2p5);
    }

    uart_puts("LEO DONE\r\n");
    for (;;) {
    }
}
