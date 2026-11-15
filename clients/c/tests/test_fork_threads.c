/* A child forked while other threads of the parent look up finishes its own
 * lookup: nothing the parent's threads held inside the library at the fork
 * may block it. As many threads as cores (at least 4) look up in a loop while
 * the main thread forks 300 times; a child that has not exited after 5 seconds is killed and fails the
 * test. */
#include "common.h"

#include <pthread.h>
#include <signal.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define MIN_THREADS 4
#define MAX_THREADS 64
#define FORKS 300
#define CHILD_TIMEOUT_S 5

static volatile int stop;

static void *worker(void *arg) {
    (void)arg;
    char out[EMFOR_ADDR_MAX];
    while (!stop) CHECK(emfor_lookup("api.svc.emfor", NULL, out, sizeof out) == EMFOR_OK);
    return NULL;
}

static double now_s(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}

int main(void) {
    char out[EMFOR_ADDR_MAX];
    CHECK(emfor_lookup("api.svc.emfor", NULL, out, sizeof out) == EMFOR_OK);
    /* At least 4 lookup threads, and as many as the machine has cores where
     * the platform exposes the count: FreeBSD hides _SC_NPROCESSORS_ONLN
     * under -D_POSIX_C_SOURCE, so it runs 4 there (the Rust test uses
     * available_parallelism on every platform). */
#ifdef _SC_NPROCESSORS_ONLN
    long cores = sysconf(_SC_NPROCESSORS_ONLN);
#else
    long cores = MIN_THREADS;
#endif
    int threads = cores < MIN_THREADS ? MIN_THREADS : cores > MAX_THREADS ? MAX_THREADS : (int)cores;
    pthread_t t[MAX_THREADS];
    for (int i = 0; i < threads; i++) CHECK(pthread_create(&t[i], NULL, worker, NULL) == 0);

    int hung = -1;
    for (int n = 0; n < FORKS && hung < 0; n++) {
        pid_t pid = fork();
        CHECK(pid >= 0);
        if (pid == 0) {
            int ok = emfor_lookup("api.svc.emfor", NULL, out, sizeof out) == EMFOR_OK && is_backend(out);
            _exit(ok ? 0 : 1);
        }
        double start = now_s();
        int status = 0;
        for (;;) {
            pid_t r = waitpid(pid, &status, WNOHANG);
            CHECK(r >= 0);
            if (r == pid) break;
            if (now_s() - start > CHILD_TIMEOUT_S) {
                kill(pid, SIGKILL);
                waitpid(pid, &status, 0);
                hung = n;
                break;
            }
            struct timespec ms = {0, 1000000};
            nanosleep(&ms, NULL);
        }
        if (hung < 0) CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    }
    stop = 1;
    for (int i = 0; i < threads; i++) CHECK(pthread_join(t[i], NULL) == 0);
    if (hung >= 0) {
        fprintf(stderr, "child %d hung for %d s in its lookup\n", hung, CHILD_TIMEOUT_S);
        return 1;
    }
    printf("PASS fork_threads\n");
    return 0;
}
