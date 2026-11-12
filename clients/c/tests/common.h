/* Shared helpers for the C tests. The store comes from EMFOR_STORE, written
 * by emfor-fixture from tests/fixture.json (see the Makefile). */
#ifndef EMFOR_TEST_COMMON_H
#define EMFOR_TEST_COMMON_H

#include <emfor.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define BACKEND_A "10.0.0.11:8080"
#define BACKEND_B "10.0.0.12:8080"

#define CHECK(cond)                                                       \
    do {                                                                  \
        if (!(cond)) {                                                    \
            fprintf(stderr, "%s:%d: CHECK failed: %s (last error: %s)\n", \
                    __FILE__, __LINE__, #cond, emfor_last_error());       \
            exit(1);                                                      \
        }                                                                 \
    } while (0)

static inline int is_backend(const char *addr) {
    return strcmp(addr, BACKEND_A) == 0 || strcmp(addr, BACKEND_B) == 0;
}

#endif
