/* Lookup program for the conformance driver (tools/emfor-fixture,
 * emfor-conformance): <name> <affinity-key or ""> <count>. Prints one line
 * per lookup: "OK <address>" or "ERR <CODE> <message>". */
#include <emfor.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "usage: %s <name> <affinity-key> <count>\n", argv[0]);
        return 2;
    }
    int count = atoi(argv[3]);
    char addr[EMFOR_ADDR_MAX];
    for (int i = 0; i < count; i++) {
        emfor_status st = emfor_lookup(argv[1], argv[2], addr, sizeof addr);
        if (st == EMFOR_OK)
            printf("OK %s\n", addr);
        else
            printf("ERR %s %s\n", emfor_status_name(st), emfor_last_error());
    }
    return 0;
}
