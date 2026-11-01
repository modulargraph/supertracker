#!/usr/bin/env python3
"""Run the explicitly bounded vendor test and accept success only after normal exit.

This invokes the safe default suite. Explicit --nks-diagnostic requires a separate
guarded diagnostic binary, then opts into Massive X/NKS execution.
Third-party plug-in execution is separate from the portable deterministic suite.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

parser = argparse.ArgumentParser()
parser.add_argument("--executable", required=True, type=Path)
parser.add_argument("--output", required=True, type=Path)
parser.add_argument("--timeout", type=float, default=600)
selection = parser.add_mutually_exclusive_group()
selection.add_argument("--nks-diagnostic", action="store_true",
                    help="Require the separate fatal-guarded NKS diagnostic binary and its NKS result.")
selection.add_argument("--fixture-bundle", type=Path,
                    help="Run only the original deterministic VST3 fixture through the production audio graph.")
selection.add_argument("--capture-fixture-bundle", type=Path,
                    help="Run only the original asynchronous state fixture through the production capture protocol.")
args = parser.parse_args()
if not math.isfinite(args.timeout) or not 0 < args.timeout <= 3600:
    parser.error("timeout must be finite, greater than zero and no more than 3600 seconds")
root = args.output.resolve()
fixture_bundle = args.capture_fixture_bundle or args.fixture_bundle
fixture_scope = "original-vst3-capture" if args.capture_fixture_bundle else "original-vst3-fixture"
folder = root / ("vst3-capture" if args.capture_fixture_bundle else "vst3-fixture" if args.fixture_bundle else "vst3")
folder.mkdir(parents=True, exist_ok=True)
report_path = folder / "metrics.json"
log_path = folder / "process.log"
initial = {"passed": False, "audioChecksPassed": False, "processExitPassed": False,
           "status": "launching", "nksStatus": "notRun", "checks": [], "failures": []}
report_path.write_text(json.dumps(initial, indent=2) + "\n")
log_path.write_bytes(b"")
started = time.monotonic()
exit_code = None
reason = None


def verify_diagnostic_artifact(executable):
    """Reject an ordinary app before executing even the diagnostic proof flag."""
    manifest_path = executable.parent / "nks-diagnostic-artifact.json"
    source_path = executable.parent / "source-manifest.json"
    artifact = json.loads(manifest_path.read_text())
    source_bytes = source_path.read_bytes()
    sources = json.loads(source_bytes)
    required = {"tests/NativeNKSMain.swift", "tests/NativeNKSGuard.cpp", "tests/vst3_signal_guard.h"}
    if (not isinstance(artifact, dict) or not isinstance(sources, dict) or
            artifact.get("schema") != 1 or artifact.get("kind") != "SuperTrackerNKSDiagnostic" or
            artifact.get("executable") != executable.name or
            artifact.get("entryPoint") != "tests/NativeNKSMain.swift" or
            artifact.get("fatalGuard") != "tests/NativeNKSGuard.cpp" or
            artifact.get("productionAppEntryExcluded") is not True or
            "Sources/SuperTracker/App.swift" in sources or not required.issubset(sources) or
            artifact.get("sourceManifestSHA256") != hashlib.sha256(source_bytes).hexdigest() or
            executable.stat().st_size > 256 * 1024 * 1024 or
            artifact.get("executableSHA256") != hashlib.sha256(executable.read_bytes()).hexdigest()):
        raise RuntimeError("The selected executable has no matching isolated diagnostic build proof; no process was launched.")


def signal_child_group(child, requested_signal):
    # start_new_session creates an exclusive group for this run and its scanner
    # children. A timed-out parent must not leave a stuck factory helper orphaned.
    try:
        os.killpg(child.pid, requested_signal)
    except ProcessLookupError:
        pass


try:
    if args.nks_diagnostic or fixture_bundle:
        verify_diagnostic_artifact(args.executable.resolve())
        proof = subprocess.run([str(args.executable.resolve()), "--diagnostic-guard-check"],
                               stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=10)
        if proof.returncode != 0 or proof.stdout.strip() != "SUPERTRACKER_NKS_DIAGNOSTIC_GUARD=1":
            raise RuntimeError("The selected executable is not the guarded NKS diagnostic; no plug-in test was launched.")
    with log_path.open("wb") as log:
        mode = "--vst3-capture-self-test" if args.capture_fixture_bundle else "--vst3-fixture-self-test" if args.fixture_bundle else "--vst3-self-test"
        command = [str(args.executable.resolve()), mode, str(root)]
        if args.nks_diagnostic:
            command.append("--include-nks")
        if fixture_bundle:
            command.extend(["--fixture-bundle", str(fixture_bundle.resolve())])
        child = subprocess.Popen(command,
                                 stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                 env={**os.environ, "SUPERTRACKER_VST3_TRACE": "1"}, start_new_session=True)
        try:
            exit_code = child.wait(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            reason = "Native VST3 test exceeded its execution deadline."
            signal_child_group(child, signal.SIGTERM)
            try:
                exit_code = child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                signal_child_group(child, signal.SIGKILL)
                exit_code = child.wait(timeout=5)
            finally:
                # The parent may exit on TERM while a factory helper ignores it.
                signal_child_group(child, signal.SIGKILL)
except Exception as error:
    reason = str(error)
try:
    report = json.loads(report_path.read_text())
    if not isinstance(report, dict):
        raise ValueError("Native report is not an object")
except (OSError, ValueError):
    report = initial.copy()
    reason = reason or "Native VST3 test did not produce a valid report."
if not isinstance(report.get("failures"), list) or not all(isinstance(value, str) for value in report["failures"]):
    report["failures"] = ["Native report contains an invalid failures list."]
if not isinstance(report.get("checks"), list) or not all(isinstance(value, str) for value in report["checks"]):
    report["checks"] = []
    report["failures"].append("Native report contains an invalid checks list.")
audio_passed = (report.get("audioChecksPassed") is True and report.get("status") == "audioChecksComplete"
                and bool(report["checks"]) and not report["failures"])
report["audioChecksPassed"] = audio_passed
report["processExitPassed"] = exit_code == 0 and reason is None
report["processExitCode"] = exit_code
report["processSeconds"] = round(time.monotonic() - started, 3)
report["processLog"] = str(log_path)
report["nksDiagnosticRequested"] = args.nks_diagnostic
report["fixtureRequested"] = fixture_bundle is not None
report["captureFixtureRequested"] = args.capture_fixture_bundle is not None
nks_passed = not args.nks_diagnostic or report.get("nksStatus") == "passed"
measurements = report.get("measurements")
fixture_passed = not fixture_bundle or (report.get("scope") == fixture_scope and
    isinstance(measurements, dict) and measurements.get("installedVendorsUsed") is False)
if args.capture_fixture_bundle:
    fixture_passed = fixture_passed and isinstance(measurements, dict) and measurements.get("fixtureBundle") == str(fixture_bundle.resolve())
report["passed"] = audio_passed and report["processExitPassed"] and nks_passed and fixture_passed
report["status"] = "complete" if report["passed"] else "failed"
if reason:
    report.setdefault("failures", []).append(reason)
elif exit_code != 0:
    report.setdefault("failures", []).append(f"Native VST3 test did not exit normally with code zero ({exit_code}).")
elif not nks_passed:
    report.setdefault("failures", []).append("The requested NKS diagnostic did not report successful NKS verification.")
elif not fixture_passed:
    report.setdefault("failures", []).append("The requested original-fixture diagnostic did not report its isolated fixture scope.")
elif not audio_passed and not report["failures"]:
    report["failures"].append("Native VST3 test did not report completed, nonempty, successful audio checks.")
temporary = report_path.with_suffix(".json.tmp")
temporary.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
os.replace(temporary, report_path)
print(json.dumps({key: report.get(key) for key in ["passed", "audioChecksPassed", "processExitPassed", "processExitCode", "nksStatus", "checks", "failures", "processSeconds"]}, indent=2))
sys.exit(0 if report["passed"] else 1)
