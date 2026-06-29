# iolinki Reentrant Device Stack Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign the C `iolinki` device stack so one host process can run several isolated real IO-Link devices against one or more real C master ports, without fake protocol shims or prefixed duplicate builds.

**Architecture:** Add an explicit `iolink_device_t` instance API and move current singleton state behind it. Preserve the existing global `iolink_*` API as a compatibility wrapper around one static instance, then update LabWired native conformance to instantiate several real device stacks in the same process.

**Tech Stack:** C99, CMake/CTest, CMocka, existing `iolinki` C stack, `iolinki-master` C master, LabWired Rust `iolink-native` feature.

---

## Scope And Stop Gates

This plan has two implementation roots:

- Device stack: `/home/andrii/projects/labwired/core/.worktrees/iolink-simulator-conformance/third_party/iolinki`
- LabWired integration: `/home/andrii/projects/labwired/core/.worktrees/iolink-simulator-conformance`

The device stack submodule is detached in this worktree. Before implementation, create a dedicated branch or worktree for the upstream `iolinki` repo and make the LabWired submodule point at the resulting commit only after device-stack tests pass.

Stop gates:

- Do not change `iolinki-master` public behavior to hide device-stack problems.
- Do not use prefixed duplicate C builds as the final architecture.
- Do not claim "multi-device LabWired conformance" until one process runs at least two independent `iolink_device_t` instances with separate PD, ISDU writable tags, device info, events, and data-storage state.
- Keep legacy `iolink_init()`, `iolink_process()`, `iolink_pd_input_update()`, and related APIs working until all examples/tests are migrated.

## File Structure

Device-stack files:

- Create `third_party/iolinki/include/iolinki/device.h`: public reentrant instance API and `iolink_device_t` type.
- Modify `third_party/iolinki/include/iolinki/iolink.h`: include `device.h` and document legacy singleton wrappers.
- Modify `third_party/iolinki/include/iolinki/application.h`: add instance-aware PD APIs while preserving existing wrappers.
- Modify `third_party/iolinki/include/iolinki/device_info.h`: add `iolink_device_info_ctx_t` and context APIs.
- Modify `third_party/iolinki/include/iolinki/params.h`: add `iolink_params_ctx_t` and context APIs.
- Modify `third_party/iolinki/include/iolinki/isdu.h`: add device backlink in `iolink_isdu_ctx_t`.
- Modify `third_party/iolinki/include/iolinki/data_storage.h`: add parameter callbacks to `iolink_ds_ctx_t`.
- Modify `third_party/iolinki/src/iolink_core.c`: implement `iolink_device_*` and legacy wrappers.
- Modify `third_party/iolinki/src/device_info.c`: move globals into context-backed implementation.
- Modify `third_party/iolinki/src/params.c`: move NVM shadow and device-info dependency into context-backed implementation.
- Modify `third_party/iolinki/src/isdu.c`: route parameter/device-info access through the owning device instance.
- Modify `third_party/iolinki/src/data_storage.c`: route DS image build/apply through parameter callbacks instead of global functions.
- Modify `third_party/iolinki/tests/CMakeLists.txt`: add reentrant tests.
- Create `third_party/iolinki/tests/test_reentrant_device.c`: proves several device instances in one process.

LabWired files:

- Modify `crates/core/native/iolink_conformance.c`: replace singleton real-device helper with several `iolink_device_t` instances.
- Modify `crates/core/src/peripherals/components/iolink_master.rs`: add/assert multi-device native conformance result.
- Modify `crates/core/build.rs`: compile new `device.h` dependencies if needed; keep POSIX define.

## Task 1: Add Public Reentrant Device API As A Red Test

**Files:**

- Create: `third_party/iolinki/include/iolinki/device.h`
- Create: `third_party/iolinki/tests/test_reentrant_device.c`
- Modify: `third_party/iolinki/tests/CMakeLists.txt`

- [ ] **Step 1: Create the failing public-header test**

Add this file:

```c
/* third_party/iolinki/tests/test_reentrant_device.c */
#include <setjmp.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include <cmocka.h>

#include "iolinki/device.h"

typedef struct
{
    uint8_t rx[64];
    uint8_t rx_head;
    uint8_t rx_len;
    uint8_t tx[64];
    uint8_t tx_len;
    int wakeup;
} test_phy_t;

static int noop(void)
{
    return 0;
}

static void test_device_instance_api_initializes_two_devices(void** state)
{
    (void)state;
    test_phy_t phy_a_state = {0};
    test_phy_t phy_b_state = {0};
    iolink_device_t dev_a;
    iolink_device_t dev_b;
    static const iolink_phy_api_t phy = {
        .init = noop,
    };
    iolink_config_t cfg_a = {
        .m_seq_type = IOLINK_M_SEQ_TYPE_1_1,
        .min_cycle_time = 10U,
        .pd_in_len = 1U,
        .pd_out_len = 0U,
        .t_pd_us = 0U,
    };
    iolink_config_t cfg_b = {
        .m_seq_type = IOLINK_M_SEQ_TYPE_2_1,
        .min_cycle_time = 10U,
        .pd_in_len = 2U,
        .pd_out_len = 2U,
        .t_pd_us = 0U,
    };

    (void)phy_a_state;
    (void)phy_b_state;

    assert_int_equal(iolink_device_init(&dev_a, &phy, &cfg_a), 0);
    assert_int_equal(iolink_device_init(&dev_b, &phy, &cfg_b), 0);
    assert_int_equal(iolink_device_get_pd_in_len(&dev_a), 1U);
    assert_int_equal(iolink_device_get_pd_out_len(&dev_a), 0U);
    assert_int_equal(iolink_device_get_pd_in_len(&dev_b), 2U);
    assert_int_equal(iolink_device_get_pd_out_len(&dev_b), 2U);
}

int main(void)
{
    const struct CMUnitTest tests[] = {
        cmocka_unit_test(test_device_instance_api_initializes_two_devices),
    };
    return cmocka_run_group_tests(tests, NULL, NULL);
}
```

- [ ] **Step 2: Register the failing test**

In `third_party/iolinki/tests/CMakeLists.txt`, add inside the `if(CMOCKA_FOUND)` block:

```cmake
    add_iolink_test(test_reentrant_device test_reentrant_device.c)
```

- [ ] **Step 3: Verify the test fails because the API does not exist**

Run:

```bash
cmake -S third_party/iolinki -B /tmp/iolinki-reentrant-build -DBUILD_TESTING=ON
cmake --build /tmp/iolinki-reentrant-build --target test_reentrant_device
```

Expected: build fails with an error like `iolinki/device.h: No such file or directory` or unknown type `iolink_device_t`.

- [ ] **Step 4: Add the public header skeleton**

Create `third_party/iolinki/include/iolinki/device.h`:

```c
#ifndef IOLINK_DEVICE_H
#define IOLINK_DEVICE_H

#include "iolinki/application.h"
#include "iolinki/data_storage.h"
#include "iolinki/device_info.h"
#include "iolinki/dll.h"
#include "iolinki/iolink.h"
#include "iolinki/params.h"

typedef struct
{
    iolink_dll_ctx_t dll;
    iolink_config_t config;
    iolink_params_ctx_t params;
    iolink_device_info_ctx_t device_info;
    const iolink_app_callbacks_t* app_callbacks;
    iolink_reset_handler_t reset_handler;
    void* user;
} iolink_device_t;

int iolink_device_init(iolink_device_t* dev,
                       const iolink_phy_api_t* phy,
                       const iolink_config_t* config);
void iolink_device_process(iolink_device_t* dev);
int iolink_device_pd_input_update(iolink_device_t* dev,
                                  const uint8_t* data,
                                  size_t len,
                                  bool valid);
int iolink_device_pd_output_read(iolink_device_t* dev, uint8_t* data, size_t len);
void iolink_device_app_register(iolink_device_t* dev, const iolink_app_callbacks_t* callbacks);
void iolink_device_set_reset_handler(iolink_device_t* dev, iolink_reset_handler_t handler);
iolink_events_ctx_t* iolink_device_get_events_ctx(iolink_device_t* dev);
iolink_ds_ctx_t* iolink_device_get_ds_ctx(iolink_device_t* dev);
iolink_dll_state_t iolink_device_get_state(const iolink_device_t* dev);
uint8_t iolink_device_get_pd_in_len(const iolink_device_t* dev);
uint8_t iolink_device_get_pd_out_len(const iolink_device_t* dev);

#endif
```

- [ ] **Step 5: Re-run and verify the next failure**

Run:

```bash
cmake --build /tmp/iolinki-reentrant-build --target test_reentrant_device
```

Expected: build now fails on missing `iolink_params_ctx_t`, `iolink_device_info_ctx_t`, or undefined `iolink_device_*` symbols.

- [ ] **Step 6: Commit the red API test**

```bash
git -C third_party/iolinki add include/iolinki/device.h tests/test_reentrant_device.c tests/CMakeLists.txt
git -C third_party/iolinki commit -m "test: define reentrant device API expectations"
```

## Task 2: Add Context Types Without Changing Legacy Behavior

**Files:**

- Modify: `third_party/iolinki/include/iolinki/params.h`
- Modify: `third_party/iolinki/include/iolinki/device_info.h`
- Modify: `third_party/iolinki/src/params.c`
- Modify: `third_party/iolinki/src/device_info.c`

- [ ] **Step 1: Add context structs to public headers**

In `third_party/iolinki/include/iolinki/device_info.h`, add after `iolink_device_info_t`:

```c
typedef struct
{
    const iolink_device_info_t* info;
    iolink_device_info_t default_info;
    char application_tag[33];
    uint16_t access_locks;
} iolink_device_info_ctx_t;

void iolink_device_info_ctx_init(iolink_device_info_ctx_t* ctx,
                                 const iolink_device_info_t* info);
const iolink_device_info_t* iolink_device_info_ctx_get(const iolink_device_info_ctx_t* ctx);
int iolink_device_info_ctx_set_application_tag(iolink_device_info_ctx_t* ctx,
                                               const char* tag,
                                               uint8_t len);
uint16_t iolink_device_info_ctx_get_access_locks(const iolink_device_info_ctx_t* ctx);
void iolink_device_info_ctx_set_access_locks(iolink_device_info_ctx_t* ctx, uint16_t locks);
```

In `third_party/iolinki/include/iolinki/params.h`, add:

```c
#include "iolinki/device_info.h"

typedef struct
{
    char application_tag[33];
    char function_tag[33];
    char location_tag[33];
    bool application_tag_valid;
    bool function_tag_valid;
    bool location_tag_valid;
    iolink_device_info_ctx_t* device_info;
} iolink_params_ctx_t;

void iolink_params_ctx_init(iolink_params_ctx_t* ctx, iolink_device_info_ctx_t* device_info);
int iolink_params_ctx_get(iolink_params_ctx_t* ctx,
                          uint16_t index,
                          uint8_t subindex,
                          uint8_t* buffer,
                          size_t max_len);
int iolink_params_ctx_set(iolink_params_ctx_t* ctx,
                          uint16_t index,
                          uint8_t subindex,
                          const uint8_t* data,
                          size_t len,
                          bool persist);
void iolink_params_ctx_factory_reset(iolink_params_ctx_t* ctx);
```

- [ ] **Step 2: Implement context-backed device-info functions**

In `third_party/iolinki/src/device_info.c`, keep the existing globals, but implement the context functions first. Use the current default values from `g_default_info`:

```c
void iolink_device_info_ctx_init(iolink_device_info_ctx_t* ctx,
                                 const iolink_device_info_t* info)
{
    if(ctx == NULL) {
        return;
    }
    memset(ctx, 0, sizeof(*ctx));
    ctx->default_info = g_default_info;
    memcpy(ctx->application_tag, "DefaultTag", 10U);
    ctx->default_info.application_tag = ctx->application_tag;
    ctx->access_locks = ctx->default_info.access_locks;
    ctx->info = (info != NULL) ? info : &ctx->default_info;
}
```

Then implement:

```c
const iolink_device_info_t* iolink_device_info_ctx_get(const iolink_device_info_ctx_t* ctx);
int iolink_device_info_ctx_set_application_tag(iolink_device_info_ctx_t* ctx,
                                               const char* tag,
                                               uint8_t len);
uint16_t iolink_device_info_ctx_get_access_locks(const iolink_device_info_ctx_t* ctx);
void iolink_device_info_ctx_set_access_locks(iolink_device_info_ctx_t* ctx, uint16_t locks);
```

Make existing legacy functions call the context functions on a static legacy context. Preserve the old public behavior.

- [ ] **Step 3: Implement context-backed params functions**

In `third_party/iolinki/src/params.c`, introduce context implementations and make legacy functions call a static legacy context:

```c
static iolink_device_info_ctx_t g_legacy_device_info;
static iolink_params_ctx_t g_legacy_params;
static bool g_legacy_params_init;
```

Legacy init:

```c
void iolink_params_init(void)
{
    if(!g_legacy_params_init) {
        iolink_device_info_ctx_init(&g_legacy_device_info, NULL);
        iolink_params_ctx_init(&g_legacy_params, &g_legacy_device_info);
        g_legacy_params_init = true;
    }
}
```

Context init:

```c
void iolink_params_ctx_init(iolink_params_ctx_t* ctx, iolink_device_info_ctx_t* device_info)
{
    if(ctx == NULL) {
        return;
    }
    memset(ctx, 0, sizeof(*ctx));
    ctx->device_info = device_info;
}
```

Move the existing get/set/factory-reset switch logic into `iolink_params_ctx_get`, `iolink_params_ctx_set`, and `iolink_params_ctx_factory_reset`.

- [ ] **Step 4: Verify legacy tests still pass**

Run:

```bash
cmake --build /tmp/iolinki-reentrant-build
ctest --test-dir /tmp/iolinki-reentrant-build --output-on-failure
```

Expected: existing tests pass; `test_reentrant_device` may still fail on undefined `iolink_device_*` until Task 3.

- [ ] **Step 5: Commit context type migration**

```bash
git -C third_party/iolinki add include/iolinki/params.h include/iolinki/device_info.h src/params.c src/device_info.c
git -C third_party/iolinki commit -m "refactor: add context-backed device parameters"
```

## Task 3: Implement `iolink_device_t` Core And Legacy Wrappers

**Files:**

- Modify: `third_party/iolinki/include/iolinki/iolink.h`
- Modify: `third_party/iolinki/include/iolinki/application.h`
- Modify: `third_party/iolinki/src/iolink_core.c`
- Test: `third_party/iolinki/tests/test_reentrant_device.c`

- [ ] **Step 1: Update headers for compatibility**

In `third_party/iolinki/include/iolinki/iolink.h`, include the new header near the end of declarations without creating a recursive include loop. If needed, forward declare `struct iolink_device` in `device.h` and move full struct layout into `src/iolink_core.c`.

In `third_party/iolinki/include/iolinki/application.h`, add instance APIs:

```c
struct iolink_device;
typedef struct iolink_device iolink_device_t;

int iolink_device_pd_input_update(iolink_device_t* dev,
                                  const uint8_t* data,
                                  size_t len,
                                  bool valid);
int iolink_device_pd_output_read(iolink_device_t* dev, uint8_t* data, size_t len);
void iolink_device_app_register(iolink_device_t* dev, const iolink_app_callbacks_t* callbacks);
```

- [ ] **Step 2: Implement device init and process**

In `third_party/iolinki/src/iolink_core.c`, replace singleton globals with:

```c
static iolink_device_t g_legacy_device;
static bool g_legacy_device_initialized;
```

Implement:

```c
int iolink_device_init(iolink_device_t* dev,
                       const iolink_phy_api_t* phy,
                       const iolink_config_t* config)
{
    if((dev == NULL) || (phy == NULL)) {
        return -1;
    }

    memset(dev, 0, sizeof(*dev));
    if(config != NULL) {
        memcpy(&dev->config, config, sizeof(dev->config));
    } else {
        dev->config.m_seq_type = IOLINK_M_SEQ_TYPE_0;
        dev->config.min_cycle_time = 0U;
    }

    if(phy->init != NULL) {
        int err = phy->init();
        if(err != 0) {
            return err;
        }
    }

    iolink_device_info_ctx_init(&dev->device_info, NULL);
    iolink_params_ctx_init(&dev->params, &dev->device_info);
    iolink_dll_init(&dev->dll, phy);
    dev->dll.owner = dev;
    dev->dll.state_cb = core_state_cb;
    dev->dll.m_seq_type = (uint8_t)dev->config.m_seq_type;
    dev->dll.pd_in_len = dev->config.pd_in_len;
    dev->dll.pd_out_len = dev->config.pd_out_len;
    dev->dll.min_cycle_time_us = (uint32_t)dev->config.min_cycle_time * 100U;
    dev->dll.t_pd_delay_us = dev->config.t_pd_us;
    return 0;
}
```

If `iolink_dll_ctx_t` does not yet have `owner`, add `void* owner;` to `third_party/iolinki/include/iolinki/dll.h`.

- [ ] **Step 3: Implement instance operations**

In `third_party/iolinki/src/iolink_core.c`, implement:

```c
void iolink_device_process(iolink_device_t* dev)
{
    if(dev == NULL) {
        return;
    }
    iolink_dll_process(&dev->dll);
    if(dev->dll.isdu.reset_pending) {
        dev->dll.isdu.reset_pending = false;
        if(dev->reset_handler != NULL) {
            dev->reset_handler(IOLINK_RESET_DEVICE);
        }
    }
    if(dev->dll.isdu.app_reset_pending) {
        dev->dll.isdu.app_reset_pending = false;
        if(dev->reset_handler != NULL) {
            dev->reset_handler(IOLINK_RESET_APPLICATION);
        }
    }
}
```

Move the existing PD input/output and callback behavior from global `g_dll_ctx` to `dev->dll`.

- [ ] **Step 4: Make legacy wrappers call the static instance**

Implement wrappers like:

```c
int iolink_init(const iolink_phy_api_t* phy, const iolink_config_t* config)
{
    int ret = iolink_device_init(&g_legacy_device, phy, config);
    g_legacy_device_initialized = (ret == 0);
    return ret;
}

void iolink_process(void)
{
    if(g_legacy_device_initialized) {
        iolink_device_process(&g_legacy_device);
    }
}
```

Repeat for:

```c
iolink_pd_input_update
iolink_pd_output_read
iolink_app_register
iolink_set_reset_handler
iolink_get_events_ctx
iolink_get_ds_ctx
iolink_get_state
iolink_get_phy_mode
iolink_get_baudrate
iolink_get_dll_stats
iolink_set_timing_enforcement
iolink_set_t_ren_limit_us
iolink_get_m_seq_type
iolink_get_pd_in_len
iolink_get_pd_out_len
iolink_set_pd_length
```

- [ ] **Step 5: Verify the API smoke test passes**

Run:

```bash
cmake --build /tmp/iolinki-reentrant-build --target test_reentrant_device
/tmp/iolinki-reentrant-build/tests/test_reentrant_device
```

Expected: `test_device_instance_api_initializes_two_devices` passes.

- [ ] **Step 6: Verify full legacy suite**

Run:

```bash
ctest --test-dir /tmp/iolinki-reentrant-build --output-on-failure
```

Expected: all existing tests pass.

- [ ] **Step 7: Commit core instance API**

```bash
git -C third_party/iolinki add include/iolinki/device.h include/iolinki/iolink.h include/iolinki/application.h include/iolinki/dll.h src/iolink_core.c
git -C third_party/iolinki commit -m "feat: add reentrant IO-Link device instances"
```

## Task 4: Route ISDU Through Owning Device Context

**Files:**

- Modify: `third_party/iolinki/include/iolinki/isdu.h`
- Modify: `third_party/iolinki/src/dll.c`
- Modify: `third_party/iolinki/src/isdu.c`
- Test: `third_party/iolinki/tests/test_reentrant_device.c`

- [ ] **Step 1: Add a failing two-device ISDU test**

Append to `third_party/iolinki/tests/test_reentrant_device.c`:

```c
static void test_two_devices_keep_application_tags_isolated(void** state)
{
    (void)state;
    iolink_device_t dev_a;
    iolink_device_t dev_b;
    static const iolink_phy_api_t phy = {.init = noop};
    iolink_config_t cfg = {
        .m_seq_type = IOLINK_M_SEQ_TYPE_2_2,
        .min_cycle_time = 10U,
        .pd_in_len = 2U,
        .pd_out_len = 2U,
        .t_pd_us = 0U,
    };
    const uint8_t tag_a[] = "DeviceA";
    const uint8_t tag_b[] = "DeviceB";
    uint8_t out_a[32] = {0};
    uint8_t out_b[32] = {0};

    assert_int_equal(iolink_device_init(&dev_a, &phy, &cfg), 0);
    assert_int_equal(iolink_device_init(&dev_b, &phy, &cfg), 0);

    assert_int_equal(iolink_params_ctx_set(&dev_a.params, IOLINK_IDX_APPLICATION_TAG, 0U,
                                           tag_a, sizeof(tag_a) - 1U, true), 0);
    assert_int_equal(iolink_params_ctx_set(&dev_b.params, IOLINK_IDX_APPLICATION_TAG, 0U,
                                           tag_b, sizeof(tag_b) - 1U, true), 0);
    assert_int_equal(iolink_params_ctx_get(&dev_a.params, IOLINK_IDX_APPLICATION_TAG, 0U,
                                           out_a, sizeof(out_a)), (int)(sizeof(tag_a) - 1U));
    assert_int_equal(iolink_params_ctx_get(&dev_b.params, IOLINK_IDX_APPLICATION_TAG, 0U,
                                           out_b, sizeof(out_b)), (int)(sizeof(tag_b) - 1U));
    assert_memory_equal(out_a, tag_a, sizeof(tag_a) - 1U);
    assert_memory_equal(out_b, tag_b, sizeof(tag_b) - 1U);
}
```

Add it to the `tests[]` array.

- [ ] **Step 2: Verify it fails before ISDU/context routing is complete**

Run:

```bash
cmake --build /tmp/iolinki-reentrant-build --target test_reentrant_device
/tmp/iolinki-reentrant-build/tests/test_reentrant_device
```

Expected: failure if params still share global state, or compile failure if context fields are not exposed correctly.

- [ ] **Step 3: Add owner pointers**

In `third_party/iolinki/include/iolinki/isdu.h`, add to `iolink_isdu_ctx_t`:

```c
void* device_ctx;
```

In `third_party/iolinki/include/iolinki/dll.h`, add to `iolink_dll_ctx_t`:

```c
void* owner;
```

In `iolink_device_init`, set:

```c
dev->dll.owner = dev;
dev->dll.isdu.device_ctx = dev;
dev->dll.isdu.event_ctx = &dev->dll.events;
dev->dll.isdu.ds_ctx = &dev->dll.ds;
dev->dll.isdu.dll_ctx = &dev->dll;
```

- [ ] **Step 4: Replace global parameter/device-info calls in ISDU**

In `third_party/iolinki/src/isdu.c`, add:

```c
#include "iolinki/device.h"

static iolink_device_t* isdu_device(iolink_isdu_ctx_t* ctx)
{
    return (ctx == NULL) ? NULL : (iolink_device_t*)ctx->device_ctx;
}
```

Replace calls:

```c
iolink_params_get(...)
iolink_params_set(...)
iolink_params_factory_reset()
iolink_device_info_get()
iolink_device_info_get_access_locks()
iolink_device_info_set_access_locks(...)
```

with the matching context calls through `isdu_device(ctx)`.

- [ ] **Step 5: Verify isolated tag test and full suite**

Run:

```bash
cmake --build /tmp/iolinki-reentrant-build --target test_reentrant_device
/tmp/iolinki-reentrant-build/tests/test_reentrant_device
ctest --test-dir /tmp/iolinki-reentrant-build --output-on-failure
```

Expected: reentrant device test and legacy suite pass.

- [ ] **Step 6: Commit ISDU context routing**

```bash
git -C third_party/iolinki add include/iolinki/isdu.h include/iolinki/dll.h src/dll.c src/isdu.c tests/test_reentrant_device.c
git -C third_party/iolinki commit -m "refactor: route ISDU through device context"
```

## Task 5: Make Data Storage Instance-Isolated

**Files:**

- Modify: `third_party/iolinki/include/iolinki/data_storage.h`
- Modify: `third_party/iolinki/src/data_storage.c`
- Modify: `third_party/iolinki/src/iolink_core.c`
- Test: `third_party/iolinki/tests/test_reentrant_device.c`

- [ ] **Step 1: Add failing DS isolation test**

Append to `third_party/iolinki/tests/test_reentrant_device.c`:

```c
static void test_two_devices_build_distinct_data_storage_images(void** state)
{
    (void)state;
    iolink_device_t dev_a;
    iolink_device_t dev_b;
    static const iolink_phy_api_t phy = {.init = noop};
    iolink_config_t cfg = {
        .m_seq_type = IOLINK_M_SEQ_TYPE_2_2,
        .min_cycle_time = 10U,
        .pd_in_len = 2U,
        .pd_out_len = 2U,
        .t_pd_us = 0U,
    };
    const uint8_t tag_a[] = "DS-A";
    const uint8_t tag_b[] = "DS-B";
    const uint8_t* image_a;
    const uint8_t* image_b;
    size_t len_a = 0U;
    size_t len_b = 0U;

    assert_int_equal(iolink_device_init(&dev_a, &phy, &cfg), 0);
    assert_int_equal(iolink_device_init(&dev_b, &phy, &cfg), 0);
    assert_int_equal(iolink_params_ctx_set(&dev_a.params, IOLINK_IDX_APPLICATION_TAG, 0U,
                                           tag_a, sizeof(tag_a) - 1U, true), 0);
    assert_int_equal(iolink_params_ctx_set(&dev_b.params, IOLINK_IDX_APPLICATION_TAG, 0U,
                                           tag_b, sizeof(tag_b) - 1U, true), 0);

    image_a = iolink_ds_get_image(iolink_device_get_ds_ctx(&dev_a), &len_a);
    image_b = iolink_ds_get_image(iolink_device_get_ds_ctx(&dev_b), &len_b);

    assert_non_null(image_a);
    assert_non_null(image_b);
    assert_true(len_a > 0U);
    assert_true(len_b > 0U);
    assert_int_not_equal(memcmp(image_a, image_b, len_a < len_b ? len_a : len_b), 0);
}
```

- [ ] **Step 2: Add parameter callbacks to DS context**

In `third_party/iolinki/include/iolinki/data_storage.h`, add:

```c
typedef struct
{
    int (*get)(void* user, uint16_t index, uint8_t subindex, uint8_t* buffer, size_t max_len);
    int (*set)(void* user,
               uint16_t index,
               uint8_t subindex,
               const uint8_t* data,
               size_t len,
               bool persist);
    void* user;
} iolink_ds_params_api_t;
```

Add to `iolink_ds_ctx_t`:

```c
iolink_ds_params_api_t params;
```

Add a new init variant:

```c
void iolink_ds_set_params_api(iolink_ds_ctx_t* ctx, const iolink_ds_params_api_t* params);
```

- [ ] **Step 3: Wire DS callbacks from device init**

In `third_party/iolinki/src/iolink_core.c`, add static adapters:

```c
static int device_params_get(void* user, uint16_t index, uint8_t subindex, uint8_t* buffer, size_t max_len)
{
    return iolink_params_ctx_get((iolink_params_ctx_t*)user, index, subindex, buffer, max_len);
}

static int device_params_set(void* user,
                             uint16_t index,
                             uint8_t subindex,
                             const uint8_t* data,
                             size_t len,
                             bool persist)
{
    return iolink_params_ctx_set((iolink_params_ctx_t*)user, index, subindex, data, len, persist);
}
```

During `iolink_device_init`, after DS init:

```c
iolink_ds_params_api_t params_api = {
    .get = device_params_get,
    .set = device_params_set,
    .user = &dev->params,
};
iolink_ds_set_params_api(&dev->dll.ds, &params_api);
```

- [ ] **Step 4: Replace global params calls in DS**

In `third_party/iolinki/src/data_storage.c`, replace direct calls to `iolink_params_get` and `iolink_params_set` with `ctx->params.get` and `ctx->params.set`. If callbacks are missing, return `-1` from image build/apply.

- [ ] **Step 5: Verify DS isolation and full suite**

Run:

```bash
cmake --build /tmp/iolinki-reentrant-build --target test_reentrant_device
/tmp/iolinki-reentrant-build/tests/test_reentrant_device
ctest --test-dir /tmp/iolinki-reentrant-build --output-on-failure
```

Expected: DS images differ when each device has a different application tag; all existing tests pass.

- [ ] **Step 6: Commit DS isolation**

```bash
git -C third_party/iolinki add include/iolinki/data_storage.h src/data_storage.c src/iolink_core.c tests/test_reentrant_device.c
git -C third_party/iolinki commit -m "refactor: isolate data storage per device instance"
```

## Task 6: Add Real Master Against Two Real Device Instances In C

**Files:**

- Create: `third_party/iolinki/tests/test_multi_device_real_master.c`
- Modify: `third_party/iolinki/tests/CMakeLists.txt`

- [ ] **Step 1: Add failing multi-device master test**

Create `third_party/iolinki/tests/test_multi_device_real_master.c` by adapting the queue pattern from `/home/andrii/projects/iolinki-master/tests/test_master_real_iolinki_device.c`, but replace global `iolink_init()` and `iolink_process()` with two `iolink_device_t` instances:

```c
typedef struct
{
    link_queue_t master_to_device;
    link_queue_t device_to_master;
    int wakeup_pending;
    iolink_device_t device;
    iolink_master_port_t master;
    uint8_t last_pd_out[32];
    uint8_t last_pd_out_len;
} port_pair_t;
```

The test must initialize two `port_pair_t` objects:

```c
static void test_two_master_ports_drive_two_real_device_instances(void** state)
```

Expected assertions:

```c
assert_int_equal(iolink_master_get_state(&pair_a.master), IOLINK_MASTER_STATE_OPERATE);
assert_int_equal(iolink_master_get_state(&pair_b.master), IOLINK_MASTER_STATE_OPERATE);
assert_memory_equal(pd_in_a, expected_a, pd_in_len_a);
assert_memory_equal(pd_in_b, expected_b, pd_in_len_b);
assert_memory_not_equal(pd_in_a, pd_in_b, min_len);
assert_memory_equal(pair_a.last_pd_out, expected_pd_out_a, pd_out_len_a);
assert_memory_equal(pair_b.last_pd_out, expected_pd_out_b, pd_out_len_b);
```

- [ ] **Step 2: Register the test**

In `third_party/iolinki/tests/CMakeLists.txt`, add:

```cmake
    add_iolink_test(test_multi_device_real_master test_multi_device_real_master.c)
```

If `iolinki-master` is not available inside the device-stack test build, add a CMake cache variable:

```cmake
set(IOLINKI_MASTER_DIR "" CACHE PATH "Path to iolinki-master checkout")
if(IOLINKI_MASTER_DIR)
    add_executable(test_multi_device_real_master test_multi_device_real_master.c test_helpers.c)
    target_include_directories(test_multi_device_real_master PRIVATE "${IOLINKI_MASTER_DIR}/include")
    target_sources(test_multi_device_real_master PRIVATE
        "${IOLINKI_MASTER_DIR}/src/master_controller.c"
        "${IOLINKI_MASTER_DIR}/src/master_isdu.c"
        "${IOLINKI_MASTER_DIR}/src/master_parameters.c"
        "${IOLINKI_MASTER_DIR}/src/master_port.c"
        "${IOLINKI_MASTER_DIR}/src/master_sio.c"
    )
    target_link_libraries(test_multi_device_real_master iolinki ${CMOCKA_LIBRARIES})
    add_test(NAME test_multi_device_real_master COMMAND test_multi_device_real_master)
endif()
```

- [ ] **Step 3: Verify the real multi-device C test passes**

Run:

```bash
cmake -S third_party/iolinki -B /tmp/iolinki-reentrant-build -DBUILD_TESTING=ON -DIOLINKI_MASTER_DIR=/home/andrii/projects/iolinki-master
cmake --build /tmp/iolinki-reentrant-build --target test_multi_device_real_master
/tmp/iolinki-reentrant-build/tests/test_multi_device_real_master
ctest --test-dir /tmp/iolinki-reentrant-build --output-on-failure
```

Expected: both independent master/device pairs reach OPERATE in the same process.

- [ ] **Step 4: Commit real C multi-device conformance**

```bash
git -C third_party/iolinki add tests/test_multi_device_real_master.c tests/CMakeLists.txt
git -C third_party/iolinki commit -m "test: prove multi-device real master conformance"
```

## Task 7: Update LabWired Native Conformance To Use Several Real Devices

**Files:**

- Modify: `crates/core/native/iolink_conformance.c`
- Modify: `crates/core/src/peripherals/components/iolink_master.rs`
- Modify: `crates/core/build.rs`

- [ ] **Step 1: Add failing Rust assertion for several devices**

In `crates/core/src/peripherals/components/iolink_master.rs`, extend the native FFI result with:

```rust
#[cfg(test)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NativeMultiDeviceConformanceResult {
    pub(crate) port_count: u8,
    pub(crate) operate_count: u8,
    pub(crate) pd_in_a: [u8; 32],
    pub(crate) pd_in_b: [u8; 32],
    pub(crate) pd_out_a: [u8; 32],
    pub(crate) pd_out_b: [u8; 32],
    pub(crate) pd_in_len_a: u8,
    pub(crate) pd_in_len_b: u8,
    pub(crate) pd_out_len_a: u8,
    pub(crate) pd_out_len_b: u8,
}
```

Add FFI:

```rust
#[cfg(test)]
fn lw_iolm_conformance_run_multi_device(result: *mut NativeMultiDeviceConformanceResult) -> c_int;
```

Add test:

```rust
#[cfg(feature = "iolink-native")]
#[test]
fn native_real_master_runs_several_real_device_stack_instances() {
    use super::native::run_real_multi_device_stack_conformance;

    let result = run_real_multi_device_stack_conformance()
        .expect("real multi-device IO-Link conformance");
    assert_eq!(result.port_count, 2);
    assert_eq!(result.operate_count, 2);
    assert_ne!(
        &result.pd_in_a[..result.pd_in_len_a as usize],
        &result.pd_in_b[..result.pd_in_len_b as usize]
    );
    assert_ne!(
        &result.pd_out_a[..result.pd_out_len_a as usize],
        &result.pd_out_b[..result.pd_out_len_b as usize]
    );
}
```

- [ ] **Step 2: Verify the Rust test fails on missing C symbol**

Run:

```bash
IOLINKI_MASTER_DIR=/home/andrii/projects/iolinki-master \
IOLINKI_DEVICE_DIR=/home/andrii/projects/labwired/core/.worktrees/iolink-simulator-conformance/third_party/iolinki \
cargo test -p labwired-core --features iolink-native native_real_master_runs_several_real_device_stack_instances --lib -- --nocapture
```

Expected: link failure for `lw_iolm_conformance_run_multi_device`.

- [ ] **Step 3: Implement C multi-device helper**

In `crates/core/native/iolink_conformance.c`, replace the singleton helper internals with the same `port_pair_t` pattern from Task 6. Export:

```c
int lw_iolm_conformance_run_multi_device(lw_iolm_multi_device_result_t* result);
```

The helper must:

- Initialize two `iolink_master_port_t` instances.
- Initialize two `iolink_device_t` instances.
- Pump each pair independently in an interleaved loop.
- Assert by returned fields, not by C aborts.
- Fill `operate_count == 2` only if both master ports reach OPERATE.

- [ ] **Step 4: Run focused LabWired native test**

Run:

```bash
IOLINKI_MASTER_DIR=/home/andrii/projects/iolinki-master \
IOLINKI_DEVICE_DIR=/home/andrii/projects/labwired/core/.worktrees/iolink-simulator-conformance/third_party/iolinki \
cargo test -p labwired-core --features iolink-native native_real_master_runs_several_real_device_stack_instances --lib -- --nocapture
```

Expected: the new multi-device test passes.

- [ ] **Step 5: Run complete native selector**

Run:

```bash
IOLINKI_MASTER_DIR=/home/andrii/projects/iolinki-master \
IOLINKI_DEVICE_DIR=/home/andrii/projects/labwired/core/.worktrees/iolink-simulator-conformance/third_party/iolinki \
cargo test -p labwired-core --features iolink-native native_ --lib -- --nocapture
```

Expected: all native IO-Link tests pass.

- [ ] **Step 6: Commit LabWired multi-device native conformance**

```bash
git add crates/core/native/iolink_conformance.c crates/core/src/peripherals/components/iolink_master.rs crates/core/build.rs third_party/iolinki
git commit -m "sim: test real IO-Link master against several device instances"
```

## Task 8: CI And Documentation Cleanup

**Files:**

- Modify: `.github/workflows/core-iolink-native.yml`
- Modify: `third_party/iolinki/docs/ARCHITECTURE.md`
- Modify: `third_party/iolinki/docs/API.md`

- [ ] **Step 1: Keep GitHub-hosted native CI on the multi-device selector**

Ensure `.github/workflows/core-iolink-native.yml` still runs:

```yaml
run: cargo test -p labwired-core --features iolink-native native_ --lib -- --nocapture
```

This selector must include:

- master-only UART-boundary test
- single real device-stack profile matrix
- several real device-stack instances against real master ports

- [ ] **Step 2: Document instance API**

In `third_party/iolinki/docs/API.md`, add:

```markdown
## Reentrant Device Instances

Use `iolink_device_t` when an application, simulator, or test process needs
more than one IO-Link Device stack at a time. Each instance owns its DLL,
ISDU, Process Data, Events, Data Storage, parameters, device identity, and
application callbacks.

The legacy `iolink_init()` / `iolink_process()` API remains available as a
single-device compatibility wrapper around one internal `iolink_device_t`.
New integrations should prefer:

```c
iolink_device_t dev;
iolink_device_init(&dev, &phy, &config);
iolink_device_pd_input_update(&dev, pd, pd_len, true);
iolink_device_process(&dev);
```
```

- [ ] **Step 3: Document architecture boundary**

In `third_party/iolinki/docs/ARCHITECTURE.md`, add:

```markdown
## Device Instance Boundary

The stack is reentrant at the `iolink_device_t` boundary. A process may create
one instance per simulated or physical IO-Link Device. Platform PHY drivers may
still expose singleton hardware resources on embedded targets, but the protocol
state is not global.

The IO-Link Master stack remains a sibling project. Shared protocol helpers are
limited to frame/checksum/protocol constants and real-stack conformance tests.
```

- [ ] **Step 4: Run final local verification**

Run:

```bash
cmake -S third_party/iolinki -B /tmp/iolinki-reentrant-build -DBUILD_TESTING=ON -DIOLINKI_MASTER_DIR=/home/andrii/projects/iolinki-master
cmake --build /tmp/iolinki-reentrant-build
ctest --test-dir /tmp/iolinki-reentrant-build --output-on-failure
IOLINKI_MASTER_DIR=/home/andrii/projects/iolinki-master \
IOLINKI_DEVICE_DIR=/home/andrii/projects/labwired/core/.worktrees/iolink-simulator-conformance/third_party/iolinki \
cargo test -p labwired-core --features iolink-native native_ --lib -- --nocapture
IOLINKI_MASTER_DIR=/home/andrii/projects/iolinki-master \
IOLINKI_DEVICE_DIR=/home/andrii/projects/labwired/core/.worktrees/iolink-simulator-conformance/third_party/iolinki \
cargo check -p labwired-core --features iolink-native
git diff --check
```

Expected: all commands pass.

- [ ] **Step 5: Commit docs and CI cleanup**

```bash
git add .github/workflows/core-iolink-native.yml third_party/iolinki/docs/API.md third_party/iolinki/docs/ARCHITECTURE.md third_party/iolinki
git commit -m "docs: describe reentrant IO-Link device architecture"
```

## Self-Review

Spec coverage:

- Reentrant device stack: Tasks 1-5.
- Several real devices in one process: Task 6.
- LabWired native conformance with no fake device responses: Task 7.
- CI and public architecture docs: Task 8.

Placeholder scan:

- No placeholder requirements are present.
- Every task includes exact files, commands, expected outcomes, and commit commands.

Type consistency:

- `iolink_device_t`, `iolink_params_ctx_t`, `iolink_device_info_ctx_t`, and DS callback names are introduced before dependent tasks use them.
- Legacy APIs remain wrappers around the static device instance.
