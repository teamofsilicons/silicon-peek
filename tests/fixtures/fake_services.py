#!/usr/bin/env python3
"""Local fakes of IAM and Ting for running peek-server by hand (development only).

This is the old entry point; the fakes now live in scripts/e2e/fake_services.py
(same flags, plus --app-secret for Basic-auth and OBO signature checks and the
/_fake/* inspection routes). scripts/e2e/run-local.sh starts them together with
peek-server and an isolated peekd.

    python3 tests/fixtures/fake_services.py --iam-port 8081 --ting-port 8082
"""

import os
import runpy
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TARGET = os.path.join(HERE, "..", "..", "scripts", "e2e", "fake_services.py")

if __name__ == "__main__":
    os.environ.setdefault("FAKE_ACTOR", "si:dev-silicon")
    sys.argv[0] = TARGET
    runpy.run_path(TARGET, run_name="__main__")
