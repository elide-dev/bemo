/* Copyright 2026 Elide Technologies, Inc. SPDX-License-Identifier: Apache-2.0 */
#ifndef DOKAR_H
#define DOKAR_H
#include <stdint.h>
#if UINTPTR_MAX != UINT64_MAX
#error "Dokar requires a 64-bit target"
#endif
#ifdef __cplusplus
extern "C" {
#endif
#define DOKAR_ABI_VERSION 1
/* Query version first. No allocation, callbacks, thread affinity, or ownership transfer. */
uint32_t dokar_abi_version(void);
/* Zero until the transport data plane is migrated. Unknown bits must be ignored. */
uint64_t dokar_capabilities(void);
#ifdef __cplusplus
}
#endif
#endif
