// Benchmark-only wrk client policy. Linux LD_PRELOAD; never preload application servers.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <openssl/ssl.h>
#include <stdio.h>
#include <stdlib.h>

SSL_CTX *SSL_CTX_new(const SSL_METHOD *method) {
  typedef SSL_CTX *(*create_context)(const SSL_METHOD *);
  create_context original = (create_context)dlsym(RTLD_NEXT, "SSL_CTX_new");
  if (original == NULL) {
    fputs("Cannot resolve OpenSSL context constructor\n", stderr);
    abort();
  }
  SSL_CTX *context = original(method);
  if (context == NULL || SSL_CTX_set_min_proto_version(context, TLS1_3_VERSION) != 1 ||
      SSL_CTX_set_max_proto_version(context, TLS1_3_VERSION) != 1 ||
      SSL_CTX_set_ciphersuites(context, "TLS_AES_128_GCM_SHA256") != 1) {
    fputs("Cannot enforce benchmark TLS 1.3 / AES-128-GCM policy\n", stderr);
    abort();
  }
  return context;
}
