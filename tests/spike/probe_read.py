"""Spike 2: read the emfor store through the probe library with ctypes.

Usage: python3 probe_read.py <shared library> <store path>
Exit 0 when the heartbeat is positive.
"""
import ctypes
import sys

lib = ctypes.CDLL(sys.argv[1])
lib.probe_heartbeat.argtypes = [ctypes.c_char_p]
lib.probe_heartbeat.restype = ctypes.c_int64
hb = lib.probe_heartbeat(sys.argv[2].encode())
print("heartbeat:", hb)
if hb <= 0:
    print("FAIL probe_heartbeat returned", hb)
    sys.exit(1)
print("PASS Python")
