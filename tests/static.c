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
  return 0;
}
