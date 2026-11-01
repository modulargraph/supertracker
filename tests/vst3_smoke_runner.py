#!/usr/bin/env python3
"""Promote VST3 audio checks to a pass only after normal child process exit."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument("bundle")
parser.add_argument("preset")
parser.add_argument("prefix")
parser.add_argument("--executable", default="./test-output/vst3-smoke")
parser.add_argument("--timeout", type=float, default=45)
args = parser.parse_args()
if not 0 < args.timeout <= 60:
    parser.error("timeout must be greater than zero and no more than 60 seconds")
report_path = Path(args.prefix + ".json")
report_path.write_text(json.dumps({"passed": False, "status": "running"}) + "\n")
stage_path = Path(args.prefix + ".stage.json")
stage_path.unlink(missing_ok=True)
timeout = False
with open(args.prefix + ".log", "w") as log:
    try:
        process = subprocess.run([args.executable, args.bundle, args.preset, args.prefix],
                                 stdout=log, stderr=subprocess.STDOUT, timeout=args.timeout)
        exit_code = process.returncode
    except subprocess.TimeoutExpired:
        # subprocess.run kills and waits for its child before raising this exception.
        timeout = True
        exit_code = None
try:
    report = json.loads(stage_path.read_text())
except (OSError, ValueError):
    report = {"audioChecksPassed": False}
report["exitCode"] = exit_code
report["timedOut"] = timeout
report["processExitPassed"] = exit_code == 0 and not timeout
report["passed"] = report.get("audioChecksPassed", False) and report["processExitPassed"]
report["status"] = "passed" if report["passed"] else "failed"
if timeout:
    report["failureReason"] = "Diagnostic child exceeded the bounded timeout and was killed and reaped."
elif exit_code in (134, 138, 139):
    report["failureReason"] = f"Diagnostic fatal-signal guard contained signal {exit_code - 128}; cleanup failed."
elif exit_code is not None and exit_code < 0:
    report["failureReason"] = f"Child terminated from unhandled signal {-exit_code}."
elif not report["passed"]:
    report["failureReason"] = "Audio checks or normal child completion failed."
report_path.write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
sys.exit(0 if report["passed"] else 1)
