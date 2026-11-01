#!/usr/bin/env python3
"""Bound one explicit installed-preset UI diagnostic, preserving fatal/timeout failure.

Never accepts an ordinary app or an old unguarded smoke binary. Merely importing
this module or building its fixture does not execute vendor code.
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
import uuid

KIND = "SuperTrackerNKSUIDiagnostic"
PROOF = "SUPERTRACKER_NKS_UI_DIAGNOSTIC_GUARD=1"
SCOPE = "one-installed-vst3-nks-mapped-ui"
RUNNER_SHA256 = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()


def verify_artifact(executable):
    executable = executable.resolve()
    if any(part.lower().endswith(".app") for part in executable.parts):
        raise RuntimeError("An application bundle cannot serve as the installed-NKS diagnostic.")
    artifact = json.loads((executable.parent / "nks-ui-diagnostic-artifact.json").read_text())
    source_bytes = (executable.parent / "source-manifest.json").read_bytes()
    sources = json.loads(source_bytes)
    required = {"tests/NativeNKSUIMain.swift", "tests/NativeNKSUIAcceptance.swift", "tests/NativeNKSGuard.cpp",
                "tests/vst3_signal_guard.h", "Resources/Web/index.html", "Resources/Web/nks-controls.js",
                "Resources/Web/nks-loader.js", "Resources/Web/step-fx.js"}
    if (not isinstance(artifact, dict) or not isinstance(sources, dict) or artifact.get("schema") != 1
            or artifact.get("kind") != KIND or artifact.get("executable") != executable.name
            or artifact.get("entryPoint") != "tests/NativeNKSUIMain.swift"
            or artifact.get("fatalGuard") != "tests/NativeNKSGuard.cpp"
            or artifact.get("productionAppEntryExcluded") is not True
            or "Sources/SuperTracker/App.swift" in sources or not required.issubset(sources)
            or artifact.get("sourceManifestSHA256") != hashlib.sha256(source_bytes).hexdigest()
            or executable.stat().st_size > 256 * 1024 * 1024
            or artifact.get("executableSHA256") != hashlib.sha256(executable.read_bytes()).hexdigest()):
        raise RuntimeError("No matching isolated diagnostic build proof; no process was launched.")
    provenance_bytes = (executable.parent / "source-provenance.json").read_bytes()
    provenance = json.loads(provenance_bytes)
    if (not isinstance(provenance, dict) or provenance.get("schema") != 1
            or artifact.get("sourceProvenanceSHA256") != hashlib.sha256(provenance_bytes).hexdigest()
            or provenance.get("productionSourceRoot") != artifact.get("productionSourceRoot")
            or provenance.get("diagnosticToolingRoot") != artifact.get("diagnosticToolingRoot")
            or not isinstance(provenance.get("entries"), dict)
            or set(provenance["entries"]) != set(sources)):
        raise RuntimeError("Diagnostic production/tooling provenance is missing or inconsistent.")
    for relative, digest in sources.items():
        entry = provenance["entries"][relative]
        diagnostic = relative.startswith(("tests/", "tools/"))
        origin = "diagnostic-tooling" if diagnostic else "production"
        source_root = provenance["diagnosticToolingRoot" if diagnostic else "productionSourceRoot"]
        if (not isinstance(entry, dict) or entry.get("origin") != origin or entry.get("sha256") != digest
                or entry.get("sourcePath") != str(Path(source_root) / relative)):
            raise RuntimeError("Diagnostic source origin is inconsistent: " + relative)
    range_files = {"Sources/TrackerEngine/TrackerUnitRange.cpp", "Sources/TrackerEngine/include/TrackerUnitRange.h"}
    range_present = range_files.intersection(sources)
    if (bool(range_present) and range_present != range_files) or artifact.get("includesTrackerUnitRange") is not (range_present == range_files):
        raise RuntimeError("Diagnostic optional TrackerUnitRange source/header proof is inconsistent.")
    snapshot = Path(artifact["sourceDirectory"]).resolve()
    if not snapshot.is_relative_to(executable.parent / "snapshots"):
        raise RuntimeError("Diagnostic source snapshot is outside its isolated build directory.")
    web_root = Path(artifact["webRoot"]).resolve()
    if web_root != snapshot / "Resources/Web":
        raise RuntimeError("Diagnostic interface is not the frozen source snapshot.")
    for relative, digest in sources.items():
        path = Path(relative)
        if path.is_absolute() or ".." in path.parts or not isinstance(digest, str):
            raise RuntimeError("Malformed diagnostic source manifest.")
        source = snapshot / path
        if not source.resolve().is_relative_to(snapshot) or hashlib.sha256(source.read_bytes()).hexdigest() != digest:
            raise RuntimeError("Diagnostic source or interface changed after compilation: " + relative)
    return artifact, web_root


def signal_group(child, requested_signal):
    try:
        os.killpg(child.pid, requested_signal)
    except ProcessLookupError:
        pass


def run(executable, output, timeout):
    # External task cancellation must enter the same bounded cleanup as Ctrl-C.
    # Suppress repeated TERM while unwinding, then restore the caller's handler.
    previous = signal.getsignal(signal.SIGTERM)
    received = False
    def terminate(signum, frame):
        nonlocal received
        if not received:
            received = True
            raise InterruptedError("Diagnostic parent received SIGTERM.")
    signal.signal(signal.SIGTERM, terminate)
    try:
        return _run(executable, output, timeout)
    finally:
        signal.signal(signal.SIGTERM, previous)


def _run(executable, output, timeout):
    if not math.isfinite(timeout) or not 0 < timeout <= 900:
        raise ValueError("Deadline must be finite and within 0..900 seconds.")
    output = output.resolve()
    folder = output / "nks-ui"
    folder.mkdir(parents=True, exist_ok=True)
    report_path, log_path = folder / "report.json", folder / "process.log"
    if report_path.exists() or log_path.exists():
        raise ValueError("This output already contains a diagnostic report/log; choose a fresh directory to preserve previous evidence.")
    run_id = str(uuid.uuid4())
    initial = {"passed": False, "checksPassed": False, "processExitPassed": False, "status": "launching",
               "scope": SCOPE, "runID": run_id, "checks": [], "failures": []}
    report_path.write_text(json.dumps(initial, indent=2) + "\n")
    log_path.write_bytes(b"")
    started, exit_code, reason = time.monotonic(), None, None
    build_proof = None
    run_executable = None
    try:
        artifact, web_root = verify_artifact(executable)
        build_proof = {key: artifact[key] for key in ("executableSHA256", "sourceManifestSHA256", "sourceProvenanceSHA256", "productionSourceRoot", "diagnosticToolingRoot", "includesTrackerUnitRange", "sourceDirectory", "webRoot")}
        # Never launch the mutable build output after verification. A concurrent
        # rebuild can replace it or revoke its old artifact. Pin the exact bytes
        # to this fresh run directory and verify that copy against the proof.
        run_directory = folder / "executable"
        run_directory.mkdir(mode=0o700)
        run_executable = run_directory / KIND
        image = executable.read_bytes()
        if hashlib.sha256(image).hexdigest() != artifact["executableSHA256"]:
            raise RuntimeError("Diagnostic executable changed after verification; no process was launched.")
        descriptor = os.open(run_executable, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o500)
        with os.fdopen(descriptor, "wb") as pinned:
            pinned.write(image)
        proof = subprocess.run([str(run_executable), "--diagnostic-guard-check"], stdin=subprocess.DEVNULL,
                               capture_output=True, text=True, timeout=10, start_new_session=True)
        if proof.returncode != 0 or proof.stdout.strip() != PROOF:
            raise RuntimeError("The executable did not prove the dedicated fatal-signal guard; no vendor test was launched.")
        with log_path.open("wb") as log:
            child = subprocess.Popen([str(run_executable), "--nks-ui-self-test", str(output),
                                      "--web-root", str(web_root), "--run-id", run_id],
                                     stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                     env={**os.environ, "SUPERTRACKER_VST3_TRACE": "1"}, start_new_session=True)
            try:
                exit_code = child.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                reason = "Installed-NKS UI diagnostic exceeded its execution deadline."
            except BaseException as error:
                # Ctrl-C also needs an on-disk failed report and bounded child
                # cleanup. No interrupted run may outlive the diagnostic parent.
                reason = f"Diagnostic interrupted or child wait failed: {type(error).__name__}: {error}"
            finally:
                if exit_code is None:
                    signal_group(child, signal.SIGTERM)
                    try:
                        exit_code = child.wait(timeout=5)
                    except BaseException:
                        signal_group(child, signal.SIGKILL)
                        try:
                            exit_code = child.wait(timeout=5)
                        except BaseException as error:
                            reason = reason or f"Child cleanup failed: {type(error).__name__}: {error}"
                # Also reap scanner descendants after a normal child exit.
                signal_group(child, signal.SIGKILL)
    except BaseException as error:
        reason = f"{type(error).__name__}: {error}"
    try:
        report = json.loads(report_path.read_text())
        if not isinstance(report, dict):
            raise ValueError("Diagnostic report is not an object.")
    except (ValueError, OSError):
        report = initial.copy()
        reason = reason or "No valid native diagnostic report was produced."
    failures = report.get("failures")
    if not isinstance(failures, list) or not all(isinstance(value, str) for value in failures):
        failures = ["Native report has malformed failures."]
    checks = report.get("checks")
    if not isinstance(checks, list) or not all(isinstance(value, str) for value in checks):
        checks = []
        failures.append("Native report has malformed checks.")
    fatal = "SUPERTRACKER_DIAGNOSTIC_FATAL_SIGNAL=" in log_path.read_text(errors="replace")
    verified = (report.get("checksPassed") is True and report.get("status") == "checksComplete"
                and report.get("scope") == SCOPE and report.get("runID") == run_id and len(checks) >= 8 and not failures)
    process_passed = exit_code == 0 and reason is None and not fatal
    if reason:
        failures.append(reason)
    if fatal:
        failures.append("The diagnostic caught a fatal vendor/native signal; guarded exit remains failure.")
    if exit_code != 0:
        failures.append(f"Diagnostic did not exit normally with code zero ({exit_code}).")
    if not verified and not failures:
        failures.append("Diagnostic did not report all completed checks for this exact run and scope.")
    report["runnerSHA256"] = RUNNER_SHA256
    report["runnerPath"] = str(Path(__file__).resolve())
    if run_executable is not None:
        report["runExecutable"] = str(run_executable)
    if build_proof is not None:
        report["buildProof"] = build_proof
    report.update(passed=verified and process_passed, checksPassed=verified, processExitPassed=process_passed,
                  processExitCode=exit_code, caughtFatalSignal=fatal, processSeconds=round(time.monotonic() - started, 3),
                  processLog=str(log_path), checks=checks, failures=failures)
    report["status"] = "complete" if report["passed"] else "failed"
    temporary = report_path.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, report_path)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--timeout", type=float, default=300)
    args = parser.parse_args()
    try:
        report = run(args.executable, args.output, args.timeout)
    except (ValueError, OSError) as error:
        parser.error(str(error))
    print(json.dumps({key: report.get(key) for key in ["passed", "checksPassed", "processExitPassed", "processExitCode",
          "caughtFatalSignal", "checks", "failures", "processSeconds"]}, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
