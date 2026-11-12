/* Spike: open the emfor store read-only through the system liblmdb and
 * check that the daemon's heartbeat advances.
 * Usage: lmdb_read <store path>
 * Exit 0 when the open succeeds, meta/schema_version is "1" and the
 * heartbeat increases over three reads 1s apart. */
#include <lmdb.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int get(MDB_env *env, const char *key, char *out, size_t len) {
    MDB_txn *txn;
    MDB_dbi dbi;
    MDB_val k = {strlen(key), (void *)key}, v;
    int rc = mdb_txn_begin(env, NULL, MDB_RDONLY, &txn);
    if (rc) return rc;
    if (!(rc = mdb_dbi_open(txn, NULL, 0, &dbi)) && !(rc = mdb_get(txn, dbi, &k, &v))) {
        size_t n = v.mv_size < len - 1 ? v.mv_size : len - 1;
        memcpy(out, v.mv_data, n);
        out[n] = 0;
    }
    mdb_txn_abort(txn);
    return rc;
}

int main(int argc, char **argv) {
    if (argc != 2) { fprintf(stderr, "usage: %s <store>\n", argv[0]); return 2; }
    int maj, min, pat;
    printf("liblmdb: %s (%d.%d.%d)\n", mdb_version(&maj, &min, &pat), maj, min, pat);
    MDB_env *env;
    int rc = mdb_env_create(&env);
    if (!rc) rc = mdb_env_open(env, argv[1], MDB_RDONLY | MDB_NOSUBDIR | MDB_NOTLS, 0644);
    if (rc) { printf("FAIL open: %s (%d)\n", mdb_strerror(rc), rc); return 1; }
    char buf[64];
    if ((rc = get(env, "meta/schema_version", buf, sizeof buf))) {
        printf("FAIL schema_version: %s\n", mdb_strerror(rc)); return 1;
    }
    printf("schema_version: %s\n", buf);
    if (strcmp(buf, "1")) { printf("FAIL schema_version is not 1\n"); return 1; }
    long long prev = -1;
    int increased = 1;
    for (int i = 0; i < 3; i++) {
        if (i) sleep(1);
        if ((rc = get(env, "meta/heartbeat", buf, sizeof buf))) {
            printf("FAIL heartbeat: %s\n", mdb_strerror(rc)); return 1;
        }
        long long hb = atoll(buf);
        printf("heartbeat: %lld\n", hb);
        if (prev >= 0 && hb <= prev) increased = 0;
        prev = hb;
    }
    mdb_env_close(env);
    if (!increased) { printf("FAIL heartbeat did not increase\n"); return 1; }
    printf("PASS C\n");
    return 0;
}
