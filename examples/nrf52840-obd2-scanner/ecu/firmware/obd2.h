#ifndef OBD2_H
#define OBD2_H

#include <stdbool.h>
#include <stdint.h>

enum { OBD2_NO_FRAME = 0, OBD2_FRAME_READY = 1, OBD2_MALFORMED = -1 };

typedef struct {
    uint32_t id;
    uint8_t dlc;
    uint8_t data[8];
} obd2_frame_t;

typedef struct {
    uint8_t dtc_count;
    uint8_t vin_transfer_state;
    uint32_t vin_deadline;
} obd2_ecu_t;

void obd2_init(obd2_ecu_t *ecu);
int obd2_process(obd2_ecu_t *ecu, const obd2_frame_t *request,
                 uint32_t now_ms, obd2_frame_t *response);
int obd2_poll(obd2_ecu_t *ecu, uint32_t now_ms, obd2_frame_t *response);

#endif
