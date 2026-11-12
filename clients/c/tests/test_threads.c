/* 8 threads x 1000 lookups: every one succeeds, and round-robin spreads
 * them evenly over the two backends. */
#include "common.h"

#include <pthread.h>

#define THREADS 8
#define LOOKUPS 1000

static void *worker(void *arg) {
    int *count_a = arg;
    char out[EMFOR_ADDR_MAX];
    for (int i = 0; i < LOOKUPS; i++) {
        CHECK(emfor_lookup("api.svc.emfor", NULL, out, sizeof out) == EMFOR_OK);
        CHECK(is_backend(out));
        if (strcmp(out, BACKEND_A) == 0) (*count_a)++;
    }
    return NULL;
}

int main(void) {
    pthread_t t[THREADS];
    int count_a[THREADS] = {0};
    for (int i = 0; i < THREADS; i++)
        CHECK(pthread_create(&t[i], NULL, worker, &count_a[i]) == 0);
    int total_a = 0;
    for (int i = 0; i < THREADS; i++) {
        CHECK(pthread_join(t[i], NULL) == 0);
        total_a += count_a[i];
    }
    CHECK(total_a == THREADS * LOOKUPS / 2);
    printf("PASS threads\n");
    return 0;
}
