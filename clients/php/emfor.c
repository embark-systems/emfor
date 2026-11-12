/* PHP extension emfor: Emfor\lookup() and Emfor\LookupException over
 * libemfor (the C library built from the emfor Rust client). The store path
 * and staleness come only from EMFOR_STORE and EMFOR_MAX_STALENESS. */
#ifdef HAVE_CONFIG_H
#include "config.h"
#endif

#include "php.h"
#include "ext/standard/info.h"
#include "ext/spl/spl_exceptions.h"
#include "zend_exceptions.h"

#include <emfor.h>
#include <string.h>

#include "emfor_arginfo.h"

#define PHP_EMFOR_VERSION "0.1.0"

static zend_class_entry *emfor_lookup_exception_ce;

static int has_nul(const zend_string *s)
{
	return strlen(ZSTR_VAL(s)) != ZSTR_LEN(s);
}

ZEND_FUNCTION(Emfor_lookup)
{
	zend_string *name;
	zend_string *key = NULL;
	char out[EMFOR_ADDR_MAX];
	emfor_status st;
	const char *msg, *code;
	zend_object *ex;

	ZEND_PARSE_PARAMETERS_START(1, 2)
		Z_PARAM_STR(name)
		Z_PARAM_OPTIONAL
		Z_PARAM_STR_OR_NULL(key)
	ZEND_PARSE_PARAMETERS_END();

	if (has_nul(name)) {
		zend_argument_value_error(1, "must not contain any null bytes");
		RETURN_THROWS();
	}
	if (key && has_nul(key)) {
		zend_argument_value_error(2, "must not contain any null bytes");
		RETURN_THROWS();
	}

	st = emfor_lookup(ZSTR_VAL(name), key ? ZSTR_VAL(key) : NULL, out, sizeof out);
	if (st == EMFOR_OK) {
		RETURN_STRING(out);
	}
	/* Thread-local in libemfor; read on the thread that just failed. */
	msg = emfor_last_error();
	if (st == EMFOR_INVALID_ARGUMENT) {
		zend_value_error("%s", msg);
		RETURN_THROWS();
	}
	code = emfor_status_name(st);
	ex = zend_throw_exception_ex(emfor_lookup_exception_ce, (zend_long)st, "%s: %s", code, msg);
	zend_update_property_string(emfor_lookup_exception_ce, ex, "errorCode", sizeof("errorCode") - 1, code);
}

PHP_MINIT_FUNCTION(emfor)
{
	zend_class_entry ce;
	zval undef;
	zend_string *prop;

	INIT_NS_CLASS_ENTRY(ce, "Emfor", "LookupException", NULL);
	emfor_lookup_exception_ce = zend_register_internal_class_ex(&ce, spl_ce_RuntimeException);

	ZVAL_UNDEF(&undef);
	prop = zend_string_init_interned("errorCode", sizeof("errorCode") - 1, 1);
	zend_declare_typed_property(emfor_lookup_exception_ce, prop, &undef,
		ZEND_ACC_PUBLIC | ZEND_ACC_READONLY, NULL,
		(zend_type) ZEND_TYPE_INIT_MASK(MAY_BE_STRING));
	zend_string_release(prop);
	return SUCCESS;
}

PHP_MINFO_FUNCTION(emfor)
{
	php_info_print_table_start();
	php_info_print_table_row(2, "emfor support", "enabled");
	php_info_print_table_row(2, "Version", PHP_EMFOR_VERSION);
	php_info_print_table_end();
}

static const zend_module_dep emfor_deps[] = {
	ZEND_MOD_REQUIRED("spl")
	ZEND_MOD_END
};

zend_module_entry emfor_module_entry = {
	STANDARD_MODULE_HEADER_EX, NULL,
	emfor_deps,
	"emfor",
	ext_functions,
	PHP_MINIT(emfor),
	NULL,
	NULL,
	NULL,
	PHP_MINFO(emfor),
	PHP_EMFOR_VERSION,
	STANDARD_MODULE_PROPERTIES
};

#ifdef COMPILE_DL_EMFOR
ZEND_GET_MODULE(emfor)
#endif
