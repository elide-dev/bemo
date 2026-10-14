#include "bemo.h"
#include "elide_transport.h"
#include <assert.h>

int main(void) {
  assert(bemo_abi_version() == BEMO_ABI_VERSION);
  assert(elide_transport_abi_version() == 3);
  uint64_t owner = elide_transport_owner_new(4096);
  assert(owner != 0);
  uint64_t buffer = elide_transport_buffer_new(owner, 128);
  assert(buffer != 0);
  assert(elide_transport_owner_used(owner) == 128);
  assert(elide_transport_buffer_release(buffer) == 0);
  assert(elide_transport_owner_used(owner) == 0);
  assert(elide_transport_owner_release(owner) == 0);
  assert((bemo_capabilities() & BEMO_CAP_NATIVE_ALLOC) != 0);
  uint64_t live = bemo_alloc_live();
  unsigned char *memory = bemo_alloc(4096);
  assert(memory != 0 && ((uintptr_t)memory & 15) == 0);
  memory[0] = 1;
  memory[4095] = 2;
  assert(bemo_alloc_live() >= live + 4096);
  bemo_free(memory, 4096);
  assert(bemo_alloc(0) == 0);
  assert(bemo_alloc(UINT64_C(1) << 31) == 0);
  bemo_free(0, 4096);
  assert(bemo_alloc_live() == live);
  for (uint64_t size = 1; size <= 16; size++) {
    unsigned char *small = bemo_alloc(size);
    assert(small != 0 && ((uintptr_t)small & 15) == 0);
    assert(bemo_alloc_live() == live + size);
    for (uint64_t i = 0; i < size; i++)
      small[i] = (unsigned char)(size + i);
    assert(small[0] == (unsigned char)size);
    assert(small[size - 1] == (unsigned char)(size + (size - 1)));
    bemo_free(small, size);
    assert(bemo_alloc_live() == live);
  }
  return 0;
}
