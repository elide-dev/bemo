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
#define BEMO_CAP_NATIVE_ALLOC UINT64_C(2)
#define BEMO_CAP_HTTP1_COMPLETE_CREDIT UINT64_C(4)
/* Query version first. No allocation, callbacks, thread affinity, or ownership transfer. */
uint32_t bemo_abi_version(void);
/* Implemented data-plane capabilities. Unknown bits must be ignored. */
uint64_t bemo_capabilities(void);
/* BEMO_CAP_NATIVE_ALLOC: memory from the transport's own allocator (bundled mimalloc, the
 * embedder's mi_malloc, or its Rust global allocator), for embedder-managed buffers. Any thread.
 * Returns NULL for size 0, sizes over 2^31 - 1, or failure; alignment is at least 16. */
void* bemo_alloc(uint64_t size);
/* Free once, with the size passed to bemo_alloc. NULL is ignored. */
void bemo_free(void *pointer, uint64_t size);
/* Bytes allocated by bemo_alloc and not yet freed, process-wide. */
uint64_t bemo_alloc_live(void);
#ifdef __cplusplus
}
#endif
#endif
