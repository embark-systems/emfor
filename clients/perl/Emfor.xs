/* XS glue for Emfor.pm: one call into libemfor. Returns (status, address or
 * message); lib/Emfor.pm turns a failure into an exception. */
#define PERL_NO_GET_CONTEXT
#include "EXTERN.h"
#include "perl.h"
#include "XSUB.h"

#include <emfor.h>
#include <string.h>

#define INVALID "Emfor::lookup: invalid argument: "

MODULE = Emfor    PACKAGE = Emfor

void
_lookup(name_sv, key_sv)
    SV *name_sv
    SV *key_sv
  PPCODE:
  {
    STRLEN nlen, klen = 0;
    const char *name, *key = NULL;
    char out[EMFOR_ADDR_MAX];
    emfor_status st;
    if (!SvOK(name_sv))
        croak(INVALID "name is undef");
    name = SvPVutf8(name_sv, nlen);
    if (SvOK(key_sv))
        key = SvPVutf8(key_sv, klen);
    if (strlen(name) != nlen || (key && strlen(key) != klen))
        croak(INVALID "name or affinity key contains a NUL byte");
    st = emfor_lookup(name, key, out, sizeof out);
    EXTEND(SP, 2);
    mPUSHi((IV)st);
    if (st == EMFOR_OK) {
        mPUSHp(out, strlen(out));
    } else {
        const char *msg = emfor_last_error();
        PUSHs(sv_2mortal(newSVpvn_utf8(msg, strlen(msg), 1)));
    }
  }
