/* Written by hand to match emfor.stub.php. */

ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX(arginfo_Emfor_lookup, 0, 1, IS_STRING, 0)
	ZEND_ARG_TYPE_INFO(0, name, IS_STRING, 0)
	ZEND_ARG_TYPE_INFO_WITH_DEFAULT_VALUE(0, affinity_key, IS_STRING, 1, "null")
ZEND_END_ARG_INFO()

ZEND_FUNCTION(Emfor_lookup);

static const zend_function_entry ext_functions[] = {
	ZEND_NS_FALIAS("Emfor", lookup, Emfor_lookup, arginfo_Emfor_lookup)
	ZEND_FE_END
};
