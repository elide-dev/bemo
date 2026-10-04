#include <stdint.h>
/* No capability symbol: the loader must reject the version before resolving it. */
uint32_t dokar_abi_version(void) { return UINT32_C(999); }
