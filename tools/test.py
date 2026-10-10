#!/usr/bin/env python3
"""Integration test runner: boots the image in QEMU and drives it through QMP.

Tests live in tools/tests/test_*.py as functions `test_*(t)` taking a TestContext.
Artifacts (screenshots, serial logs) go to build/test-results/<test>/.

  tools/test.py                 # all tests
  tools/test.py boot shell      # tests whose name contains any of the words
"""
import importlib.util
import os
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "lib"))
from machine import ROOT, Machine  # noqa: E402
import pngio  # noqa: E402
from lumen import SkipTest  # noqa: E402

RESULTS = os.path.join(ROOT, "build", "test-results")


class TestFailure(AssertionError):
    pass


class TestContext:
    def __init__(self, name):
        self.name = name
        self.dir = os.path.join(RESULTS, name)
        os.makedirs(self.dir, exist_ok=True)
        self.machines = []

    def machine(self, **kw):
        kw.setdefault("workdir", os.path.join(self.dir, f"qemu{len(self.machines)}"))
        kw.setdefault("no_reboot", True)
        m = Machine(**kw)
        self.machines.append(m)
        return m.start()

    def path(self, name):
        return os.path.join(self.dir, name)

    def screenshot(self, m, name):
        path = self.path(name + ".png")
        m.screendump(path)
        return pngio.read_png(path)

    def check(self, cond, msg):
        if not cond:
            raise TestFailure(msg)

    def close(self):
        for m in self.machines:
            m.stop()


def color_close(a, b, tol=12):
    return all(abs(x - y) <= tol for x, y in zip(a, b))


def discover(filters):
    tests = []
    for fname in sorted(os.listdir(os.path.join(HERE, "tests"))):
        if not (fname.startswith("test_") and fname.endswith(".py")):
            continue
        spec = importlib.util.spec_from_file_location(fname[:-3], os.path.join(HERE, "tests", fname))
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        for attr in sorted(dir(mod)):
            if attr.startswith("test_"):
                name = f"{fname[5:-3]}.{attr[5:]}"
                if not filters or any(f in name for f in filters):
                    tests.append((name, getattr(mod, attr)))
    return tests


def main():
    tests = discover(sys.argv[1:])
    failed = []
    skipped = []
    for name, fn in tests:
        t = TestContext(name)
        start = time.monotonic()
        print(f"[ RUN  ] {name}", flush=True)
        try:
            fn(t)
        except SkipTest as e:
            skipped.append(name)
            print(f"[ SKIP ] {name}: {e}", flush=True)
            continue
        except Exception as e:  # noqa: BLE001 - report every failure kind
            failed.append(name)
            for i, m in enumerate(t.machines):
                try:
                    m.screendump(t.path(f"failure-{i}.png"))
                except Exception:  # noqa: BLE001
                    pass
            print("".join(traceback.format_exception(e)).rstrip())
            print(f"[ FAIL ] {name} ({time.monotonic() - start:.1f}s) -> {t.dir}", flush=True)
        else:
            print(f"[  OK  ] {name} ({time.monotonic() - start:.1f}s)", flush=True)
        finally:
            t.close()
    print(f"\n{len(tests) - len(failed) - len(skipped)}/{len(tests)} passed, {len(skipped)} skipped")
    if failed:
        print("failed: " + ", ".join(failed))
    sys.exit(1 if failed or not tests else 0)


if __name__ == "__main__":
    main()
