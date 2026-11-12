/* Spike 2: read the emfor store through the cargo-c probe library.
 * Usage: probe_read <store path>
 * Exit 0 when the heartbeat increases over three reads 1s apart. */
#include <emforprobe.h>
#include <stdio.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 2) { fprintf(stderr, "usage: %s <store>\n", argv[0]); return 2; }
    int64_t prev = -1;
    for (int i = 0; i < 3; i++) {
        if (i) sleep(1);
        int64_t hb = probe_heartbeat(argv[1]);
        printf("heartbeat: %lld\n", (long long)hb);
        if (hb <= 0) { printf("FAIL probe_heartbeat returned %lld\n", (long long)hb); return 1; }
        if (hb <= prev) { printf("FAIL heartbeat did not increase\n"); return 1; }
        prev = hb;
    }
    printf("PASS C\n");
    return 0;
}
