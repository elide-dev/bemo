#include "bemo.h"
#include <assert.h>

int main(void) {
  assert(bemo_abi_version() == BEMO_ABI_VERSION);
  assert(bemo_capabilities() == BEMO_CAP_TRANSPORT_V3);
  return 0;
}
