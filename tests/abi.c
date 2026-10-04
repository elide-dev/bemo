#include "dokar.h"
#include <assert.h>

int main(void) {
  assert(dokar_abi_version() == DOKAR_ABI_VERSION);
  assert(dokar_capabilities() == DOKAR_CAP_TRANSPORT_V3);
  return 0;
}
