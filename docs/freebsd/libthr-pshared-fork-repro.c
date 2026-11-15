/* Minimal libthr pshared_lock fork repro, no LMDB.
 * usage: repro <file> <parent-mode> <child-mode> [threads] [forks]
 * parent-mode: r = threads lock/unlock an existing pshared mutex (rdlock of pshared_lock only)
 *              w = threads map the file anew, lock/unlock, unmap (wrlock of pshared_lock each time)
 *              n = threads spin without touching mutexes
 * child-mode:  new = child maps the file anew and locks (needs wrlock)
 *              old = child locks through the inherited mapping (rdlock only)
 */
#include <sys/mman.h>
#include <sys/wait.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static const char *file; static char pmode; static volatile int stop;
static pthread_mutex_t *shared;

static pthread_mutex_t *map_new(void) {
    int fd = open(file, O_RDWR);
    void *p = mmap(NULL, 4096, PROT_READ|PROT_WRITE, MAP_SHARED, fd, 0);
    close(fd);
    if (p == MAP_FAILED) { perror("mmap"); _exit(3); }
    return p;
}
static void *worker(void *a) {
    (void)a;
    while (!stop) {
        if (pmode == 'r') { pthread_mutex_lock(shared); pthread_mutex_unlock(shared); }
        else if (pmode == 'w') { pthread_mutex_t *m = map_new(); pthread_mutex_lock(m); pthread_mutex_unlock(m); munmap(m, 4096); }
    }
    return NULL;
}
static double now_s(void){ struct timespec ts; clock_gettime(CLOCK_MONOTONIC,&ts); return ts.tv_sec+ts.tv_nsec/1e9; }

int main(int argc, char **argv) {
    file = argv[1]; pmode = argv[2][0]; int child_new = strcmp(argv[3], "new") == 0;
    int nthr = argc > 4 ? atoi(argv[4]) : 4, forks = argc > 5 ? atoi(argv[5]) : 300;
    int keep = getenv("KEEP") != NULL;
    int fd = open(file, O_RDWR|O_CREAT|O_TRUNC, 0600); ftruncate(fd, 4096); close(fd);
    shared = map_new();
    pthread_mutexattr_t at; pthread_mutexattr_init(&at); pthread_mutexattr_setpshared(&at, PTHREAD_PROCESS_SHARED);
    pthread_mutex_init(shared, &at);
    pthread_t t[64];
    for (int i = 0; i < nthr; i++) pthread_create(&t[i], NULL, worker, NULL);
    int hung = 0, done = 0;
    for (int n = 0; n < forks; n++) {
        pid_t pid = fork();
        if (pid == 0) {
            pthread_mutex_t *m = child_new ? map_new() : shared;
            int rc = pthread_mutex_lock(m); if (rc == 0) rc = pthread_mutex_unlock(m);
            _exit(rc ? 1 : 0);
        }
        double s = now_s(); int st = 0;
        for (;;) {
            if (waitpid(pid, &st, WNOHANG) == pid) break;
            if (now_s() - s > 2) {
                hung++;
                if (keep) { printf("HUNG child %d pid %d (left alive)\n", n, pid); fflush(stdout); stop = 1; goto out; }
                kill(pid, SIGKILL); waitpid(pid, &st, 0); st = 0; break;
            }
            struct timespec ms = {0, 200000}; nanosleep(&ms, NULL);
        }
        if (st != 0) { printf("child %d status %d\n", n, st); }
        done++;
    }
out:
    stop = 1;
    for (int i = 0; i < nthr; i++) pthread_join(t[i], NULL);
    printf("parent=%c child=%s threads=%d forks=%d hung=%d\n", pmode, argv[3], nthr, done, hung);
    return hung ? 1 : 0;
}
