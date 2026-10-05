/* Copyright 2026 Elide Technologies, Inc. SPDX-License-Identifier: Apache-2.0 */
#ifndef BEMO_H
#define BEMO_H
#include <stdint.h>
#if UINTPTR_MAX != UINT64_MAX
#error "Bemo requires a 64-bit target"
#endif
#ifdef __cplusplus
extern "C" {
#endif
#define BEMO_ABI_VERSION 1
#define BEMO_CAP_TRANSPORT_V3 UINT64_C(1)
/* Query version first. No allocation, callbacks, thread affinity, or ownership transfer. */
uint32_t bemo_abi_version(void);
/* Implemented data-plane capabilities. Unknown bits must be ignored. */
uint64_t bemo_capabilities(void);
#ifdef __cplusplus
}
#endif
#endif
