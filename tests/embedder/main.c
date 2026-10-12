/* Links the embedder's static library like Elide's final link does. */
#include "bemo.h"
#include "elide_transport.h"
#include <assert.h>
#include <stdio.h>

uint64_t embedder_live_bytes(void);

int main(void) {
  assert(bemo_abi_version() == BEMO_ABI_VERSION);
  assert((bemo_capabilities() & BEMO_CAP_TRANSPORT_V3) != 0);
  assert(elide_transport_abi_version() == 3);
  enum { SIZE = 1 << 20 };
  uint64_t owner = elide_transport_owner_new(4 * SIZE);
  assert(owner != 0);
  uint64_t before = embedder_live_bytes();
  uint64_t buffer = elide_transport_buffer_new(owner, SIZE);
  assert(buffer != 0);
  uint64_t during = embedder_live_bytes();
#ifdef BEMO_EMBEDDER_RUST_ALLOCATOR
  /* Buffer storage comes from the embedder's #[global_allocator]. */
  assert(during >= before + SIZE);
#else
  /* Buffer storage comes from mi_malloc, which the embedder (or its libc) provides. */
  assert(during < before + SIZE);
#endif
  assert(elide_transport_buffer_release(buffer) == 0);
  assert(elide_transport_owner_used(owner) == 0);
  assert(elide_transport_owner_release(owner) == 0);
  assert(embedder_live_bytes() < before + SIZE);
  puts("embedder contract passed");
  return 0;
}
