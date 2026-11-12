"""Tests for the Python wrapper. The Makefile sets EMFOR_LIBRARY and writes
clients/c/tests/fixture.json to EMFOR_STORE: api.svc.emfor, round-robin over
the two BACKENDS."""

import collections
import os
import subprocess
import sys
import threading
import unittest

import emfor

BACKENDS = {"10.0.0.11:8080", "10.0.0.12:8080"}
needs_store = unittest.skipUnless(os.environ.get("EMFOR_STORE"), "EMFOR_STORE not set; run make test")


@needs_store
class LookupTest(unittest.TestCase):
    def test_returns_a_backend_and_alternates(self):
        a = emfor.lookup("api.svc.emfor")
        b = emfor.lookup("API.svc.emfor.", affinity_key="")
        self.assertIn(a, BACKENDS)
        self.assertIn(b, BACKENDS)
        self.assertNotEqual(a, b)

    def test_not_found(self):
        with self.assertRaises(emfor.NotFoundError) as cm:
            emfor.lookup("nope.svc.emfor")
        e = cm.exception
        self.assertIsInstance(e, emfor.Error)
        self.assertEqual(e.code, "NOT_FOUND")
        self.assertTrue(e.message)
        self.assertEqual(str(e), "NOT_FOUND: " + e.message)

    def test_lookup_after_fork(self):
        emfor.lookup("api.svc.emfor")
        pid = os.fork()
        if pid == 0:
            try:
                ok = all(emfor.lookup("api.svc.emfor") in BACKENDS for _ in range(3))
            except BaseException:
                ok = False
            os._exit(0 if ok else 1)
        _, status = os.waitpid(pid, 0)
        self.assertTrue(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, status)
        self.assertIn(emfor.lookup("api.svc.emfor"), BACKENDS)

    def test_concurrent_lookups(self):
        counts = collections.Counter()
        lock = threading.Lock()
        errors = []

        def work():
            local = collections.Counter()
            try:
                for _ in range(1000):
                    local[emfor.lookup("api.svc.emfor")] += 1
            except Exception as e:  # reported below
                errors.append(e)
            with lock:
                counts.update(local)

        threads = [threading.Thread(target=work) for _ in range(8)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        self.assertEqual(errors, [])
        self.assertEqual(dict(counts), {b: 4000 for b in BACKENDS})


class ArgumentTest(unittest.TestCase):
    def test_nul_is_value_error(self):
        for args in (("api\0svc", None), ("api.svc.emfor", "a\0b")):
            with self.assertRaises(ValueError) as cm:
                emfor.lookup(*args)
            self.assertNotIsInstance(cm.exception, emfor.Error)

    def test_lone_surrogate_is_value_error(self):
        with self.assertRaises(ValueError):
            emfor.lookup("\udc80")

    def test_non_str_is_type_error(self):
        with self.assertRaises(TypeError):
            emfor.lookup(b"api.svc.emfor")
        with self.assertRaises(TypeError):
            emfor.lookup("api.svc.emfor", 42)

    def test_codes(self):
        self.assertEqual(
            [c.code for c in (emfor.UnavailableError, emfor.IncompatibleError, emfor.NotFoundError, emfor.AllDownError)],
            ["UNAVAILABLE", "INCOMPATIBLE", "NOT_FOUND", "ALL_DOWN"],
        )


class LoadTest(unittest.TestCase):
    def test_missing_library_names_the_variable(self):
        env = dict(os.environ, EMFOR_LIBRARY="/nonexistent/libemfor.so")
        here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        p = subprocess.run(
            [sys.executable, "-c", "import emfor"], cwd=here, env=env, capture_output=True, text=True
        )
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("ImportError", p.stderr)
        self.assertIn("EMFOR_LIBRARY", p.stderr)


if __name__ == "__main__":
    unittest.main()
