#include "bemo.h"
#include <assert.h>
#include <pthread.h>
#include <string.h>

enum { THREADS = 4, BLOCKS = 256, SIZE = 1024 };

/* Each thread frees the blocks another thread allocated, as Netty releases buffers anywhere. */
static unsigned char *blocks[THREADS][BLOCKS];

static void *allocate(void *argument) {
  unsigned char **row = argument;
  for (int index = 0; index < BLOCKS; index++) {
    row[index] = bemo_alloc(SIZE);
    assert(row[index] != 0);
    memset(row[index], index, SIZE);
  }
  return 0;
}

static void *release(void *argument) {
  unsigned char **row = argument;
  for (int index = 0; index < BLOCKS; index++) {
    assert(row[index][SIZE - 1] == (unsigned char)index);
    bemo_free(row[index], SIZE);
  }
  return 0;
}

static void run(void *(*body)(void *)) {
  pthread_t threads[THREADS];
  for (int index = 0; index < THREADS; index++) {
    assert(pthread_create(&threads[index], 0, body, blocks[(index + 1) % THREADS]) == 0);
  }
  for (int index = 0; index < THREADS; index++) {
    assert(pthread_join(threads[index], 0) == 0);
  }
}

int main(void) {
  assert(bemo_abi_version() == BEMO_ABI_VERSION);
  assert(bemo_capabilities() == (BEMO_CAP_TRANSPORT_V3 | BEMO_CAP_NATIVE_ALLOC | BEMO_CAP_HTTP1_COMPLETE_CREDIT));
  uint64_t live = bemo_alloc_live();
  run(allocate);
  assert(bemo_alloc_live() == live + (uint64_t)THREADS * BLOCKS * SIZE);
  run(release);
  assert(bemo_alloc_live() == live);
  return 0;
}
