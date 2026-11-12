/* Lookups against a store written by emfor-fixture, and the status codes. */
#include "common.h"

int main(void) {
    char out[EMFOR_ADDR_MAX];
    char first[EMFOR_ADDR_MAX];

    CHECK(emfor_lookup("api.svc.emfor", NULL, first, sizeof first) == EMFOR_OK);
    CHECK(is_backend(first));
    /* Round-robin: the next lookup (name normalized) gets the other backend. */
    CHECK(emfor_lookup("API.svc.emfor.", NULL, out, sizeof out) == EMFOR_OK);
    CHECK(is_backend(out) && strcmp(out, first) != 0);

    CHECK(emfor_lookup("nope.svc.emfor", NULL, out, sizeof out) == EMFOR_NOT_FOUND);
    CHECK(strcmp(emfor_status_name(EMFOR_NOT_FOUND), "NOT_FOUND") == 0);
    CHECK(strlen(emfor_last_error()) > 0);

    /* Too small a buffer: rejected and left unchanged. */
    char small[8] = "#######";
    CHECK(emfor_lookup("api.svc.emfor", NULL, small, sizeof small) == EMFOR_INVALID_ARGUMENT);
    CHECK(strcmp(small, "#######") == 0);
    CHECK(emfor_lookup(NULL, NULL, out, sizeof out) == EMFOR_INVALID_ARGUMENT);
    CHECK(strcmp(emfor_status_name(EMFOR_INVALID_ARGUMENT), "INVALID_ARGUMENT") == 0);

    printf("PASS lookup\n");
    return 0;
}
