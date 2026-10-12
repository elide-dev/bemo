#include <stdint.h>
#include <stdlib.h>
/*
 * Deliberate defects for the sanitized JVM stage. A clean contract run only counts
 * as evidence when these are reported under the same runtime, options, and loader.
 */

/* Heap overflow one byte past an 8-byte allocation: ASAN must report it. */
int32_t bemo_canary_overflow(void) {
  volatile char *bytes = malloc(8);
  bytes[8] = 1;
  free((void *)bytes);
  return 0;
}

/* Unsynchronized counter written from several Java threads: TSAN must report it. */
static uint64_t counter;

void bemo_canary_race(int32_t iterations) {
  for (int32_t index = 0; index < iterations; index++) {
    counter++;
  }
}
