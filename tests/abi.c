#include "dokar.h"
#include <assert.h>

int main(void) {
  assert(dokar_abi_version() == DOKAR_ABI_VERSION);
  assert(dokar_capabilities() == UINT64_C(0));
  return 0;
}
