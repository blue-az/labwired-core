#include "obd2.h"

#include <stddef.h>

#define REQUEST_ID 0x7DFu
#define FLOW_CONTROL_ID 0x7E0u
#define RESPONSE_ID 0x7E8u
#define VIN_TIMEOUT_MS 1000u

static void clear_frame(obd2_frame_t *frame)
{
    frame->id = RESPONSE_ID;
    frame->dlc = 8u;
    for (uint8_t i = 0; i < 8u; ++i) frame->data[i] = 0u;
}

static int negative(obd2_frame_t *response, uint8_t service, uint8_t nrc)
{
    clear_frame(response);
    response->data[0] = 3u;
    response->data[1] = 0x7Fu;
    response->data[2] = service;
    response->data[3] = nrc;
    return OBD2_FRAME_READY;
}

void obd2_init(obd2_ecu_t *ecu)
{
    ecu->dtc_count = 2u;
    ecu->vin_transfer_state = 0u;
    ecu->vin_deadline = 0u;
}

static int mode01(const obd2_frame_t *request, obd2_frame_t *response)
{
    if (request->data[0] != 2u) return OBD2_MALFORMED;
    clear_frame(response);
    response->data[1] = 0x41u;
    response->data[2] = request->data[2];
    switch (request->data[2]) {
    case 0x00u:
        response->data[0] = 6u;
        /* SAE J1979: PID 01 is bit 31, so 05/0C/0D => 0x08180000. */
        response->data[3] = 0x08u;
        response->data[4] = 0x18u;
        return OBD2_FRAME_READY;
    case 0x05u:
        response->data[0] = 3u;
        response->data[3] = 130u; /* 90 C + 40 */
        return OBD2_FRAME_READY;
    case 0x0Cu:
        response->data[0] = 4u;
        response->data[3] = 0x2Eu;
        response->data[4] = 0xE0u; /* 3000 RPM * 4 */
        return OBD2_FRAME_READY;
    case 0x0Du:
        response->data[0] = 3u;
        response->data[3] = 88u;
        return OBD2_FRAME_READY;
    default:
        return negative(response, 0x01u, 0x12u);
    }
}

static int mode03(const obd2_ecu_t *ecu, const obd2_frame_t *request,
                  obd2_frame_t *response)
{
    if (request->data[0] != 1u) return OBD2_MALFORMED;
    clear_frame(response);
    response->data[1] = 0x43u;
    if (ecu->dtc_count == 0u) {
        response->data[0] = 1u;
    } else {
        response->data[0] = 5u;
        response->data[2] = 0x01u;
        response->data[3] = 0x33u;
        response->data[4] = 0xC1u;
        response->data[5] = 0x23u;
    }
    return OBD2_FRAME_READY;
}

static int mode04(obd2_ecu_t *ecu, const obd2_frame_t *request,
                  obd2_frame_t *response)
{
    if (request->data[0] != 1u) return OBD2_MALFORMED;
    clear_frame(response);
    response->data[0] = 1u;
    response->data[1] = 0x44u;
    ecu->dtc_count = 0u;
    return OBD2_FRAME_READY;
}

static int mode09(obd2_ecu_t *ecu, const obd2_frame_t *request, uint32_t now_ms,
                  obd2_frame_t *response)
{
    if (request->data[0] != 2u) return OBD2_MALFORMED;
    if (request->data[2] != 0x02u) return negative(response, 0x09u, 0x12u);
    clear_frame(response);
    const uint8_t ff[8] = {0x10u, 0x14u, 0x49u, 0x02u, 0x01u, 'L', 'W', 'O'};
    for (uint8_t i = 0; i < 8u; ++i) response->data[i] = ff[i];
    ecu->vin_transfer_state = 1u;
    ecu->vin_deadline = now_ms + VIN_TIMEOUT_MS;
    return OBD2_FRAME_READY;
}

int obd2_process(obd2_ecu_t *ecu, const obd2_frame_t *request,
                 uint32_t now_ms, obd2_frame_t *response)
{
    if (request->id == FLOW_CONTROL_ID) {
        if (ecu->vin_transfer_state != 1u) return OBD2_NO_FRAME;
        if (request->dlc != 8u || request->data[0] != 0x30u ||
            (int32_t)(now_ms - ecu->vin_deadline) > 0) {
            ecu->vin_transfer_state = 0u;
            return OBD2_MALFORMED;
        }
        clear_frame(response);
        const uint8_t cf1[8] = {0x21u, 'B', 'D', '2', 'S', 'I', 'M', '0'};
        for (uint8_t i = 0; i < 8u; ++i) response->data[i] = cf1[i];
        ecu->vin_transfer_state = 2u;
        return OBD2_FRAME_READY;
    }
    if (request->id != REQUEST_ID) return OBD2_NO_FRAME;
    if (request->dlc != 8u || request->data[0] == 0u ||
        request->data[0] > 7u || request->data[0] >= request->dlc)
        return OBD2_MALFORMED;

    switch (request->data[1]) {
    case 0x01u: return mode01(request, response);
    case 0x03u: return mode03(ecu, request, response);
    case 0x04u: return mode04(ecu, request, response);
    case 0x09u: return mode09(ecu, request, now_ms, response);
    default: return negative(response, request->data[1], 0x11u);
    }
}

int obd2_poll(obd2_ecu_t *ecu, uint32_t now_ms, obd2_frame_t *response)
{
    if (ecu->vin_transfer_state == 1u &&
        (int32_t)(now_ms - ecu->vin_deadline) > 0) {
        ecu->vin_transfer_state = 0u;
        return OBD2_NO_FRAME;
    }
    if (ecu->vin_transfer_state != 2u) return OBD2_NO_FRAME;
    clear_frame(response);
    const uint8_t cf2[8] = {0x22u, '0', '0', '0', '0', '0', '0', '1'};
    for (uint8_t i = 0; i < 8u; ++i) response->data[i] = cf2[i];
    ecu->vin_transfer_state = 0u;
    return OBD2_FRAME_READY;
}
