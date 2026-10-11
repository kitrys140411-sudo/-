#!/usr/bin/env python3
"""Compositor test runner: runs `lumen --backend headless` with test clients.

Runs inside the build container (needs the musl binaries and Mesa's llvmpipe).
Tests live in tools/ctests/test_*.py as functions `test_*(t)`; artifacts
(screenshots, logs) go to build/ctest-results/<test>/.

  tools/ctest.py                # all tests (release binaries)
  tools/ctest.py overview       # tests whose name contains any of the words
  LUMEN_PROFILE=debug tools/ctest.py
"""
import importlib.util
import os
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(HERE, "lib"))
from headless import Compositor  # noqa: E402

PROFILE = os.environ.get("LUMEN_PROFILE", "release")
BIN = os.path.join(ROOT, "build", "target", "x86_64-unknown-linux-musl", PROFILE)
RESULTS = os.path.join(ROOT, "build", "ctest-results")


class TestFailure(AssertionError):
    pass


class TestContext:
    def __init__(self, name):
        self.name = name
        self.dir = os.path.join(RESULTS, name)
        self.compositors = []

    def compositor(self, **kw):
        n = len(self.compositors)
        c = Compositor(os.path.join(self.dir, f"c{n}"), os.path.join(BIN, "lumen"),
                       os.path.join(BIN, "lumen-testclient"), **kw)
        self.compositors.append(c)
        return c

    def check(self, cond, msg):
        if not cond:
            raise TestFailure(msg)

    def log(self, msg):
        print(f"         {msg}", flush=True)

    def close(self):
        for c in self.compositors:
            c.close()


def discover(filters):
    tests = []
    for fname in sorted(os.listdir(os.path.join(HERE, "ctests"))):
        if not (fname.startswith("test_") and fname.endswith(".py")):
            continue
        spec = importlib.util.spec_from_file_location(fname[:-3], os.path.join(HERE, "ctests", fname))
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        # Keep definition order: tests build on each other conceptually.
        for attr, fn in vars(mod).items():
            if attr.startswith("test_") and callable(fn):
                name = f"{fname[5:-3]}.{attr[5:]}"
                if not filters or any(f in name for f in filters):
                    tests.append((name, fn))
    return tests


def main():
    for b in ("lumen", "lumen-testclient"):
        if not os.path.exists(os.path.join(BIN, b)):
            sys.exit(f"missing {os.path.join(BIN, b)} (build first)")
    tests = discover(sys.argv[1:])
    failed = []
    for name, fn in tests:
        t = TestContext(name)
        start = time.monotonic()
        print(f"[ RUN  ] {name}", flush=True)
        try:
            fn(t)
        except Exception as e:  # noqa: BLE001 - report every failure kind
            failed.append(name)
            for i, c in enumerate(t.compositors):
                try:
                    c.screenshot(f"failure-{i}")
                except Exception:  # noqa: BLE001
                    pass
            print("".join(traceback.format_exception(e)).rstrip())
            print(f"[ FAIL ] {name} ({time.monotonic() - start:.1f}s) -> {t.dir}", flush=True)
        else:
            print(f"[  OK  ] {name} ({time.monotonic() - start:.1f}s)", flush=True)
        finally:
            t.close()
    print(f"\n{len(tests) - len(failed)}/{len(tests)} passed")
    if failed:
        print("failed: " + ", ".join(failed))
    sys.exit(1 if failed or not tests else 0)


if __name__ == "__main__":
    main()
