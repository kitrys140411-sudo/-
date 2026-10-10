#!/usr/bin/env python3
"""Run the Lumen image in QEMU (interactive or headless).

  tools/qemu.py run [--headless] [--virgl] [--persistent]
  tools/qemu.py screenshot OUT.png [--after SECONDS]
"""
import argparse
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "lib"))
from machine import Machine  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    run = sub.add_parser("run")
    run.add_argument("--headless", action="store_true")
    run.add_argument("--virgl", action="store_true")
    run.add_argument("--snapshot", action="store_true", help="discard disk writes")
    shot = sub.add_parser("screenshot")
    shot.add_argument("out")
    shot.add_argument("--after", type=float, default=20.0)
    args = ap.parse_args()

    if args.cmd == "run":
        headless = args.headless or not (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY"))
        m = Machine(headless=headless, virgl=args.virgl, snapshot=args.snapshot)
        print(" ".join(m.args()), file=sys.stderr)
        m.start()
        print(f"serial log: {m.workdir}/serial.log  (QMP: {m.qmp_path})", file=sys.stderr)
        try:
            # Mirror the serial console to stdout until QEMU exits.
            offset = 0
            while m.proc.poll() is None:
                time.sleep(0.1)
                text = m.serial_buf[offset:]
                if text:
                    sys.stdout.write(text.decode("utf-8", "replace"))
                    sys.stdout.flush()
                    offset += len(text)
        except KeyboardInterrupt:
            pass
        finally:
            m.stop()
    elif args.cmd == "screenshot":
        with Machine() as m:
            time.sleep(args.after)
            m.screendump(args.out)
            print(args.out)


if __name__ == "__main__":
    main()
