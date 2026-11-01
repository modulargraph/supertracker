#!/usr/bin/env python3
"""The native-run wrapper must reject a success report followed by abnormal exit."""
import json
import contextlib
import hashlib
import io
import os
from pathlib import Path
import signal
import runpy
import shutil
import subprocess
import sys
import tempfile
import time
from unittest.mock import patch

runner = Path(__file__).resolve().parents[1] / "tools/run_vst3_native_test.py"


def diagnostic_proof(child):
    sources = {name: "fixture" for name in ["tests/NativeNKSMain.swift", "tests/NativeNKSGuard.cpp", "tests/vst3_signal_guard.h"]}
    source_path = child.parent / "source-manifest.json"
    source_path.write_text(json.dumps(sources))
    (child.parent / "nks-diagnostic-artifact.json").write_text(json.dumps({
        "schema": 1, "kind": "SuperTrackerNKSDiagnostic", "executable": child.name,
        "entryPoint": "tests/NativeNKSMain.swift", "fatalGuard": "tests/NativeNKSGuard.cpp",
        "productionAppEntryExcluded": True,
        "executableSHA256": hashlib.sha256(child.read_bytes()).hexdigest(),
        "sourceManifestSHA256": hashlib.sha256(source_path.read_bytes()).hexdigest()
    }))


with tempfile.TemporaryDirectory(prefix="supertracker-vst3-wrapper-") as temporary:
    folder = Path(temporary)
    for name, code, audio_ok in [("good", 0, True), ("exit-error", 2, True), ("audio-error", 0, False)]:
        child = folder / name
        child.write_text("#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\n"
                         "p=Path(sys.argv[2])/'vst3'/'metrics.json'\n"
                         f"p.write_text(json.dumps({{'status': 'audioChecksComplete', 'audioChecksPassed': {audio_ok!r}, 'checks': ['fixture'], 'failures': []}}))\n"
                         f"sys.exit({code})\n")
        child.chmod(0o700)
        output = folder / (name + "-output")
        result = subprocess.run([sys.executable, str(runner), "--executable", str(child), "--output", str(output)], capture_output=True, text=True)
        report = json.loads((output / "vst3/metrics.json").read_text())
        expected = code == 0 and audio_ok
        assert report["passed"] == expected, (name, report)
        assert report["processExitPassed"] == (code == 0), (name, report)
        assert (result.returncode == 0) == expected, (name, result.stdout, result.stderr)
    for name, guarded, nks, code in [("nks-good", True, "passed", 0), ("nks-missing", True, "notRun", 0),
                                     ("nks-fatal", True, "passed", 139), ("unguarded", False, "passed", 0)]:
        child = folder / name
        launched = folder / (name + "-launched")
        child.write_text("#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\n"
                         "if '--diagnostic-guard-check' in sys.argv:\n"
                         f" print({'SUPERTRACKER_NKS_DIAGNOSTIC_GUARD=1' if guarded else 'unguarded'!r});sys.exit(0)\n"
                         "assert '--include-nks' in sys.argv\n"
                         f"Path({str(launched)!r}).touch()\n"
                         "p=Path(sys.argv[2])/'vst3'/'metrics.json'\n"
                         f"p.write_text(json.dumps({{'status': 'audioChecksComplete', 'audioChecksPassed': True, 'nksStatus': {nks!r}, 'checks': ['fixture'], 'failures': []}}))\n"
                         f"sys.exit({code})\n")
        child.chmod(0o700)
        diagnostic_proof(child)
        output = folder / (name + "-output")
        result = subprocess.run([sys.executable, str(runner), "--nks-diagnostic", "--executable", str(child),
                                 "--output", str(output)], capture_output=True, text=True)
        report = json.loads((output / "vst3/metrics.json").read_text())
        expected = guarded and nks == "passed" and code == 0
        assert report["passed"] == expected and (result.returncode == 0) == expected, (name, report)
        assert launched.exists() == guarded, (name, "unguarded executable must never reach the plug-in test")
    for name, scope, measurements, code in [
            ("fixture-good", "original-vst3-fixture", {"installedVendorsUsed": False}, 0),
            ("fixture-vendor", "original-vst3-fixture", {"installedVendorsUsed": True}, 0),
            ("fixture-scope", "installed-vendors", {"installedVendorsUsed": False}, 0),
            ("fixture-malformed", "original-vst3-fixture", [], 0),
            ("fixture-fatal", "original-vst3-fixture", {"installedVendorsUsed": False}, 139)]:
        child = folder / name
        fixture = folder / "Fixture.vst3"
        payload = {'status': 'audioChecksComplete', 'audioChecksPassed': True, 'scope': scope,
                   'measurements': measurements, 'checks': ['fixture'], 'failures': []}
        child.write_text("#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\n"
                         "if '--diagnostic-guard-check' in sys.argv:\n"
                         " print('SUPERTRACKER_NKS_DIAGNOSTIC_GUARD=1');sys.exit(0)\n"
                         "assert sys.argv[1] == '--vst3-fixture-self-test'\n"
                         "assert '--include-nks' not in sys.argv\n"
                         f"assert sys.argv[3:] == ['--fixture-bundle', {str(fixture.resolve())!r}]\n"
                         "p=Path(sys.argv[2])/'vst3-fixture'/'metrics.json'\n"
                         f"p.write_text(json.dumps({payload!r}))\n"
                         f"sys.exit({code})\n")
        child.chmod(0o700)
        diagnostic_proof(child)
        output = folder / (name + "-output")
        result = subprocess.run([sys.executable, str(runner), "--fixture-bundle", str(fixture),
                                 "--executable", str(child), "--output", str(output)], capture_output=True, text=True)
        report = json.loads((output / "vst3-fixture/metrics.json").read_text())
        expected = name == "fixture-good"
        assert report["passed"] == expected and (result.returncode == 0) == expected, (name, report, result.stderr)
        assert report["fixtureRequested"] is True and report["nksDiagnosticRequested"] is False
    for name, scope, path_matches, code in [
            ("capture-good", "original-vst3-capture", True, 0),
            ("capture-wrong-path", "original-vst3-capture", False, 0),
            ("capture-wrong-scope", "original-vst3-fixture", True, 0),
            ("capture-fatal", "original-vst3-capture", True, 139)]:
        child = folder / name
        fixture = folder / "SettledCaptureFixture.vst3"
        payload = {'status': 'audioChecksComplete', 'audioChecksPassed': True, 'scope': scope,
                   'measurements': {'installedVendorsUsed': False, 'fixtureBundle': str(fixture.resolve()) if path_matches else '/wrong/bundle.vst3'},
                   'checks': ['capture fixture'], 'failures': []}
        child.write_text("#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\n"
                         "if '--diagnostic-guard-check' in sys.argv:\n"
                         " print('SUPERTRACKER_NKS_DIAGNOSTIC_GUARD=1');sys.exit(0)\n"
                         "assert sys.argv[1] == '--vst3-capture-self-test'\n"
                         "assert '--include-nks' not in sys.argv\n"
                         f"assert sys.argv[3:] == ['--fixture-bundle', {str(fixture.resolve())!r}]\n"
                         "p=Path(sys.argv[2])/'vst3-capture'/'metrics.json'\n"
                         f"p.write_text(json.dumps({payload!r}))\n"
                         f"sys.exit({code})\n")
        child.chmod(0o700)
        diagnostic_proof(child)
        output = folder / (name + "-output")
        result = subprocess.run([sys.executable, str(runner), "--capture-fixture-bundle", str(fixture),
                                 "--executable", str(child), "--output", str(output)], capture_output=True, text=True)
        report = json.loads((output / "vst3-capture/metrics.json").read_text())
        expected = name == "capture-good"
        assert report["passed"] == expected and (result.returncode == 0) == expected, (name, report, result.stderr)
        assert report["captureFixtureRequested"] is True and report["nksDiagnosticRequested"] is False
    # An ordinary app must not be started even to ask whether it has a guard.
    for name, proof in [("missing-artifact", False), ("changed-executable", True)]:
        child = folder / name
        launched = folder / (name + "-launched")
        child.write_text(f"#!/usr/bin/env python3\nfrom pathlib import Path\nPath({str(launched)!r}).touch()\n")
        child.chmod(0o700)
        if proof:
            diagnostic_proof(child)
            child.write_text(child.read_text() + "# changed after build proof\n")
        else:
            (folder / "nks-diagnostic-artifact.json").unlink(missing_ok=True)
        output = folder / (name + "-output")
        result = subprocess.run([sys.executable, str(runner), "--nks-diagnostic", "--executable", str(child),
                                 "--output", str(output)], capture_output=True, text=True)
        report = json.loads((output / "vst3/metrics.json").read_text())
        assert result.returncode != 0 and report["passed"] is False and not launched.exists(), (name, report)
    for name, payload in [("array-report", []), ("contradictory-report", {"status": "audioChecksComplete", "audioChecksPassed": True,
                           "checks": ["fixture"], "failures": ["failed"]}),
                          ("unfinished-report", {"status": "running", "audioChecksPassed": True, "checks": ["fixture"], "failures": []}),
                          ("empty-checks", {"status": "audioChecksComplete", "audioChecksPassed": True, "checks": [], "failures": []})]:
        child = folder / name
        child.write_text("#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\n"
                         "p=Path(sys.argv[2])/'vst3'/'metrics.json'\n"
                         f"p.write_text(json.dumps({payload!r}))\n")
        child.chmod(0o700)
        output = folder / (name + "-output")
        result = subprocess.run([sys.executable, str(runner), "--executable", str(child), "--output", str(output)], capture_output=True, text=True)
        report = json.loads((output / "vst3/metrics.json").read_text())
        assert result.returncode != 0 and report["passed"] is False and report["failures"], (name, report)
    child = folder / "timeout-tree"
    heartbeat = folder / "grandchild-heartbeat"
    pidfile = folder / "grandchild-pid"
    grandchild = ("import os,signal,time\nfrom pathlib import Path\n"
                  "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
                  f"Path({str(pidfile)!r}).write_text(str(os.getpid()))\n"
                  "while True:\n"
                  f" Path({str(heartbeat)!r}).write_text(str(time.monotonic_ns()))\n time.sleep(.02)\n")
    child.write_text("#!/usr/bin/env python3\nimport subprocess,sys,time\n"
                     f"subprocess.Popen([sys.executable, '-c', {grandchild!r}])\n"
                     "while True: time.sleep(.02)\n")
    child.chmod(0o700)
    output = folder / "timeout-tree-output"
    try:
        result = subprocess.run([sys.executable, str(runner), "--executable", str(child), "--output", str(output),
                                 "--timeout", "2"], capture_output=True, text=True, timeout=12)
        report = json.loads((output / "vst3/metrics.json").read_text())
        assert result.returncode != 0 and report["passed"] is False and report["processExitPassed"] is False
        assert heartbeat.exists(), "Timeout fixture did not start its descendant"
        last = heartbeat.read_text()
        time.sleep(.15)
        assert heartbeat.read_text() == last, "Timeout left the scanner descendant running"
    finally:
        # Clean up our synthetic descendant even if a wrapper regression is caught.
        if pidfile.exists():
            try:
                os.kill(int(pidfile.read_text()), signal.SIGKILL)
            except ProcessLookupError:
                pass
print("VST3 runner verification passed: normal exit, audio/NKS coverage, malformed/incomplete reports rejected, artifact hashes checked before launch, fatal-guard proof, and timeout descendant cleanup.")

# This plug-in builder consumes only the public VST3 SDK, not the production
# Swift host. Adding VST2 support to host diagnostics must not make the original
# timing fixture depend on either plug-in host or installed plug-ins.
with tempfile.TemporaryDirectory(prefix="supertracker-vst3-timing-plan-") as temporary:
    root = runner.parents[1]
    directory = Path(temporary).resolve()
    source = directory / "sdk-only"
    sdk = source / "Vendor/vst3sdk/pluginterfaces"
    shutil.copytree(root / "Vendor/vst3sdk/pluginterfaces", sdk)
    (source / "Sources/VST3Host").mkdir(parents=True)
    shutil.copy2(root / "Sources/VST3Host/VST3SDK.cpp", source / "Sources/VST3Host/VST3SDK.cpp")
    build = directory / "build"
    calls = []
    def compile_fixture(command, **kwargs):
        assert command[:4] == ["xcrun", "--sdk", "macosx", "clang++"]
        assert "-bundle" in command and "-framework" in command
        assert (build / "compile-command.json").exists()
        calls.append(command)
        Path(command[command.index("-o") + 1]).write_bytes(b"fake original fixture; not executable")
        return subprocess.CompletedProcess(command, 0)
    with patch.object(sys, "argv", ["build_vst3_timing_fixture.py", "--build-dir", str(build), "--source-root", str(source)]), \
            patch("platform.machine", return_value="arm64"), \
            patch("subprocess.check_output", return_value="/Fixture/MacOSX.sdk\n"), \
            patch("subprocess.run", side_effect=compile_fixture), contextlib.redirect_stdout(io.StringIO()):
        runpy.run_path(str(root / "tools/build_vst3_timing_fixture.py"), run_name="__main__")
    artifact = json.loads((build / "vst3-timing-fixture-artifact.json").read_text())
    manifest = json.loads((build / "source-manifest.json").read_text())
    assert len(calls) == 1 and artifact["runtimeExecuted"] is False
    assert artifact["sourceManifestSHA256"] == hashlib.sha256((build / "source-manifest.json").read_bytes()).hexdigest()
    assert artifact["executableSHA256"] == hashlib.sha256(Path(artifact["executable"]).read_bytes()).hexdigest()
    assert manifest["Sources/VST3Host/VST3SDK.cpp"]["sourcePath"] == str(source / "Sources/VST3Host/VST3SDK.cpp")
    assert manifest["tests/vst3_timing_fixture.mm"]["origin"] == "fixture-tooling"
    assert not any("VST2Host" in path or "SuperTracker/" in path for path in manifest)
    assert {Path(argument).name for argument in calls[0] if argument.endswith((".mm", ".cpp"))} == {"vst3_timing_fixture.mm", "VST3SDK.cpp"}
print("PASS original VST3 timing fixture compiles from SDK-only frozen sources without either production host")
