dnl emfor: Emfor\lookup() through libemfor, found with pkg-config.
PHP_ARG_ENABLE([emfor],
  [whether to enable the emfor extension],
  [AS_HELP_STRING([--enable-emfor], [Enable the emfor extension])],
  [yes])

if test "$PHP_EMFOR" != "no"; then
  PKG_CHECK_MODULES([EMFOR], [emfor], [],
    [AC_MSG_ERROR([pkg-config cannot find emfor: install libemfor (clients/rust, cargo cinstall --features capi) first])])
  PHP_EVAL_INCLINE($EMFOR_CFLAGS)
  PHP_EVAL_LIBLINE($EMFOR_LIBS, EMFOR_SHARED_LIBADD)
  dnl Find libemfor where pkg-config found it, without LD_LIBRARY_PATH.
  EMFOR_LIBDIR=`$PKG_CONFIG --variable=libdir emfor`
  EMFOR_SHARED_LIBADD="$EMFOR_SHARED_LIBADD -Wl,-rpath,$EMFOR_LIBDIR"
  PHP_SUBST(EMFOR_SHARED_LIBADD)
  PHP_ADD_EXTENSION_DEP(emfor, spl)
  PHP_NEW_EXTENSION(emfor, emfor.c, $ext_shared)
fi
