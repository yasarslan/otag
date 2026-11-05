#!/usr/bin/env python3
"""Run a test suite in a unique namespace and fail if its daemons survive.

Usage: python3 scripts/check-test-daemons.py -- cargo test
       python3 scripts/check-test-daemons.py -- swift test --package-path app
The Rust and Swift fixture helpers use OTAG_TEST_RUN_DIR for socket, HOME and
state roots. This guard checks process ownership, even if a socket was deleted.
"""
import os
import pathlib
import subprocess
import sys
import tempfile
import time


def main():
    command = sys.argv[1:]
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        raise SystemExit("pass the test command after --")
    with tempfile.TemporaryDirectory(prefix="otag-suite-", dir="/tmp") as root:
        environment = dict(os.environ, OTAG_TEST_RUN_DIR=root)
        result = subprocess.run(command, env=environment)
        # Parent-loss monitoring runs at 500 ms. Give it time to finish even
        # if the suite crashed before its fixture destructors could execute.
        time.sleep(1)
        output = subprocess.check_output(
            ["ps", "-axo", "pid=,uid=,command="], text=True
        )
        leaks = []
        for row in output.splitlines():
            fields = row.split(None, 2)
            if len(fields) != 3:
                continue
            pid, uid, args = fields
            if int(uid) != os.getuid() or root not in args or " daemon " not in args:
                continue
            exe, _ = args.split(" daemon ", 1)
            if pathlib.Path(exe).name == "otag":
                leaks.append((pid, args))
        if leaks:
            for pid, args in leaks:
                print(f"LEAKED test daemon {pid}: {args}", file=sys.stderr)
            return 1
        print("Test daemon guard: no surviving test daemons", flush=True)
        return result.returncode


if __name__ == "__main__":
    sys.exit(main())
