"""Look up a healthy backend for a service in the local emfor state store.

    import emfor
    try:
        addr = emfor.lookup("api.svc.emfor", affinity_key="user-42")
    except emfor.AllDownError as e:
        ...

This wraps libemfor, the C library built from the emfor Rust client, through
ctypes. The store path and staleness come only from the environment variables
EMFOR_STORE and EMFOR_MAX_STALENESS, read on the first lookup of a process.
libemfor is loaded from the path in EMFOR_LIBRARY if set, otherwise found by
name. It must come from the same emfor release as the daemon.
"""

import ctypes
import ctypes.util
import os

__all__ = [
    "lookup",
    "Error",
    "UnavailableError",
    "IncompatibleError",
    "NotFoundError",
    "AllDownError",
]

_ADDR_MAX = 64  # EMFOR_ADDR_MAX
_INVALID_ARGUMENT = 5


class Error(Exception):
    """A failed lookup. ``code`` is the name shared by every emfor client."""

    code = "UNKNOWN"

    def __init__(self, message):
        super().__init__("%s: %s" % (self.code, message))
        self.message = message


class UnavailableError(Error):
    """The store cannot be used: missing, another LMDB format, not
    initialized, or an invalid EMFOR_MAX_STALENESS."""

    code = "UNAVAILABLE"


class IncompatibleError(Error):
    """The store's format version is not 1, or a record cannot be parsed."""

    code = "INCOMPATIBLE"


class NotFoundError(Error):
    """No service has that name."""

    code = "NOT_FOUND"


class AllDownError(Error):
    """No healthy backend, and the service's on_all_down is "closed"."""

    code = "ALL_DOWN"


_BY_STATUS = {1: UnavailableError, 2: IncompatibleError, 3: NotFoundError, 4: AllDownError}


def _load():
    path = os.environ.get("EMFOR_LIBRARY")
    if path:
        try:
            return ctypes.CDLL(path)
        except OSError as e:
            raise ImportError("cannot load libemfor from EMFOR_LIBRARY=%r: %s" % (path, e)) from None
    names = []
    found = ctypes.util.find_library("emfor")
    if found:
        names.append(found)
    names += ["libemfor.so.0", "libemfor.so", "libemfor.dylib"]
    for name in names:
        try:
            return ctypes.CDLL(name)
        except OSError:
            pass
    raise ImportError(
        "cannot find libemfor (tried %s); install it, or set EMFOR_LIBRARY to its path"
        % ", ".join(names)
    )


_lib = _load()
_lib.emfor_lookup.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_size_t]
_lib.emfor_lookup.restype = ctypes.c_int
_lib.emfor_last_error.argtypes = []
_lib.emfor_last_error.restype = ctypes.c_char_p


def lookup(name, affinity_key=None):
    """Return the address of one backend of service ``name``: ``"ip:port"``,
    or ``"[ip]:port"`` for IPv6.

    ``affinity_key`` matters only for sticky services; ``None`` or ``""`` is
    no key. Raises a subclass of :class:`Error` on failure, ``TypeError`` for
    an argument that is not ``str``, and ``ValueError`` for one containing a
    NUL character or one libemfor cannot accept.
    """
    if not isinstance(name, str):
        raise TypeError("name must be str, not %s" % type(name).__name__)
    if affinity_key is not None and not isinstance(affinity_key, str):
        raise TypeError("affinity_key must be str or None, not %s" % type(affinity_key).__name__)
    if "\0" in name or (affinity_key and "\0" in affinity_key):
        raise ValueError("name or affinity_key contains a NUL character")
    cname = name.encode("utf-8")  # a lone surrogate raises UnicodeEncodeError, a ValueError
    ckey = affinity_key.encode("utf-8") if affinity_key else None
    buf = ctypes.create_string_buffer(_ADDR_MAX)
    status = _lib.emfor_lookup(cname, ckey, buf, _ADDR_MAX)
    if status == 0:
        return buf.value.decode("ascii")
    # ctypes runs the call on this OS thread, so the thread-local message is ours.
    message = (_lib.emfor_last_error() or b"").decode("utf-8", "replace")
    if status == _INVALID_ARGUMENT:
        raise ValueError(message)
    raise _BY_STATUS.get(status, Error)(message)
