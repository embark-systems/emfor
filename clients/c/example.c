/* Look up a backend for a service and print it.
 *
 *   cc example.c $(pkg-config --cflags --libs emfor) -o example
 *   ./example api.svc.emfor [affinity-key]
 */
#include <emfor.h>
#include <stdio.h>

int main(int argc, char **argv) {
    if (argc < 2 || argc > 3) {
        fprintf(stderr, "usage: %s <service> [affinity-key]\n", argv[0]);
        return 2;
    }
    char addr[EMFOR_ADDR_MAX];
    emfor_status st = emfor_lookup(argv[1], argc == 3 ? argv[2] : NULL, addr, sizeof addr);
    if (st != EMFOR_OK) {
        fprintf(stderr, "%s: %s\n", emfor_status_name(st), emfor_last_error());
        return 1;
    }
    printf("%s\n", addr); /* "10.0.0.11:8080" or "[2001:db8::11]:8080" */
    return 0;
}
