/* A forked child can look up after its parent did, and the parent goes on. */
#include "common.h"

#include <sys/wait.h>
#include <unistd.h>

static void lookup_ok(void) {
    char out[EMFOR_ADDR_MAX];
    CHECK(emfor_lookup("api.svc.emfor", NULL, out, sizeof out) == EMFOR_OK);
    CHECK(is_backend(out));
}

int main(void) {
    lookup_ok(); /* the parent opens the store before forking */
    pid_t pid = fork();
    CHECK(pid >= 0);
    if (pid == 0) {
        for (int i = 0; i < 100; i++) lookup_ok();
        _exit(0);
    }
    int status = 0;
    CHECK(waitpid(pid, &status, 0) == pid);
    CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    lookup_ok();
    printf("PASS fork\n");
    return 0;
}
