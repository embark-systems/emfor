// Package emfor looks up a healthy backend for a service in the local emfor
// state store, through libemfor (the C library built from clients/rust).
//
//	addr, err := emfor.Lookup("api.svc.emfor", "user-42")
//	var e *emfor.Error
//	if errors.As(err, &e) && e.Code == emfor.AllDown { ... }
//
// The store path and staleness come only from the environment variables
// EMFOR_STORE and EMFOR_MAX_STALENESS, read on the first lookup. libemfor must
// come from the same emfor release as the daemon. Building needs cgo and
// `pkg-config emfor`.
package emfor

/*
#cgo pkg-config: emfor
#include <stdlib.h>
#include <emfor.h>
*/
import "C"

import (
	"errors"
	"runtime"
	"strings"
	"unsafe"
)

// Code is why a lookup failed; its String is the name shared by every emfor
// client language.
type Code int

// The four shared error codes.
const (
	Unavailable  Code = 1 // the store cannot be used (missing, other LMDB format, not initialized, invalid EMFOR_MAX_STALENESS)
	Incompatible Code = 2 // the store's format version is neither 1 nor 2, or a record cannot be parsed
	NotFound     Code = 3 // no such service
	AllDown      Code = 4 // no healthy backend and on_all_down = "closed"
)

func (c Code) String() string {
	switch c {
	case Unavailable:
		return "UNAVAILABLE"
	case Incompatible:
		return "INCOMPATIBLE"
	case NotFound:
		return "NOT_FOUND"
	case AllDown:
		return "ALL_DOWN"
	}
	return "UNKNOWN"
}

// Error is a failed lookup.
type Error struct {
	Code    Code
	Message string
}

func (e *Error) Error() string { return e.Code.String() + ": " + e.Message }

// Lookup returns the address of one backend of service name: "ip:port", or
// "[ip]:port" for IPv6. affinityKey matters only for sticky services; "" is
// no key. A failure is an *Error, except for an argument libemfor cannot take
// (a NUL byte, or not UTF-8), which is a plain error.
func Lookup(name, affinityKey string) (string, error) {
	if strings.IndexByte(name, 0) >= 0 || strings.IndexByte(affinityKey, 0) >= 0 {
		return "", errors.New("emfor: invalid argument: name or affinity key contains a NUL byte")
	}
	cname := C.CString(name)
	defer C.free(unsafe.Pointer(cname))
	var ckey *C.char
	if affinityKey != "" {
		ckey = C.CString(affinityKey)
		defer C.free(unsafe.Pointer(ckey))
	}
	var buf [C.EMFOR_ADDR_MAX]C.char
	// emfor_last_error is per OS thread: read it on the thread that failed.
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	st := C.emfor_lookup(cname, ckey, &buf[0], C.size_t(len(buf)))
	if st == C.EMFOR_OK {
		return C.GoString(&buf[0]), nil
	}
	msg := C.GoString(C.emfor_last_error())
	if st == C.EMFOR_INVALID_ARGUMENT {
		return "", errors.New("emfor: invalid argument: " + msg)
	}
	return "", &Error{Code: Code(st), Message: msg}
}
