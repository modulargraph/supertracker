#!/usr/bin/env python3
"""Validate isolated build/runner planning with fakes; never execute native vendors."""
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import runpy
import signal
import shutil
import subprocess
import sys
import tempfile
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("nks_ui_runner", ROOT / "tools/run_nks_ui_diagnostic.py")
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)

with tempfile.TemporaryDirectory(prefix="supertracker-nks-ui-plan-") as temporary:
    directory = Path(temporary).resolve()
    build = directory / "build"
    compiler_calls = []

    def compiler(command, **kwargs):
        assert command[:3] == ["xcrun", "--sdk", "macosx"]
        assert command[3] in ("clang++", "swiftc"), "Build attempted native execution"
        assert (build / "compile-commands.json").exists()
        compiler_calls.append(command)
        Path(command[command.index("-o") + 1]).write_bytes(b"not executable; fake compiler output")
        return subprocess.CompletedProcess(command, 0)

    with patch.object(sys, "argv", ["build_nks_ui_diagnostic.py", "--build-dir", str(build)]), \
            patch("platform.machine", return_value="arm64"), \
            patch("subprocess.check_output", return_value="/Fixture/MacOSX.sdk\n"), \
            patch("subprocess.run", side_effect=compiler), contextlib.redirect_stdout(io.StringIO()):
        runpy.run_path(str(ROOT / "tools/build_nks_ui_diagnostic.py"), run_name="__main__")
    assert len(compiler_calls) == 9
    artifact = json.loads((build / "nks-ui-diagnostic-artifact.json").read_text())
    executable = build / artifact["executable"]
    manifest = json.loads((build / "source-manifest.json").read_text())
    assert artifact["kind"] == runner.KIND
    assert "Sources/SuperTracker/App.swift" not in manifest
    assert "Sources/SuperTracker/NativeTerminationSaveTests.swift" not in manifest
    for required in ["tests/NativeNKSUIMain.swift", "tests/NativeNKSUIAcceptance.swift", "tests/NativeNKSGuard.cpp",
                     "tests/vst3_signal_guard.h", "Resources/Web/index.html", "Resources/Web/nks-controls.js",
                     "Resources/Web/nks-loader.js", "Resources/Web/step-fx.js"]:
        assert required in manifest
    assert "tests/NativeNKSMain.swift" not in manifest
    assert str(Path(artifact["sourceDirectory"]) / "tests/NativeNKSUIAcceptance.swift") in compiler_calls[-1]
    assert "CryptoKit" in compiler_calls[-1]
    assert artifact["includesVST2Host"] is True
    for required in ("Sources/VST2Host/VST2Host.mm", "Sources/VST2Host/include/VST2Host.h", "Sources/SuperTracker/VST2AudioUnit.swift"):
        assert required in manifest
    vst2_command = next(command for command in compiler_calls if any(argument.endswith("/VST2Host.mm") for argument in command))
    assert "-fobjc-arc" in vst2_command
    assert str(Path(artifact["sourceDirectory"]) / "Sources/VST2Host/include") in vst2_command
    assert str(build / "VST2Host.o") in compiler_calls[-1]
    assert str(build / "modules/VST2Host") in compiler_calls[-1]
    assert "VST2Host.h" in (build / "modules/VST2Host/module.modulemap").read_text()
    assert "IOKit" in compiler_calls[-1] and "IOUSBHost" in compiler_calls[-1]
    assert runner.verify_artifact(executable)[1] == Path(artifact["webRoot"])
    print("PASS isolated snapshot pins real interface/native sources, dedicated entry, guard and compile-only manifest")

    created = []

    def proof(command, **kwargs):
        assert command[1:] == ["--diagnostic-guard-check"]
        assert kwargs["timeout"] == 10
        return subprocess.CompletedProcess(command, 0, runner.PROOF + "\n", "")

    class Child:
        pid = 24680
        def __init__(self, command, **kwargs):
            created.append(command)
            assert kwargs["start_new_session"] is True
            assert command[1] == "--nks-ui-self-test"
            assert Path(command[command.index("--web-root") + 1]) == Path(artifact["webRoot"])
            output = Path(command[2]) / "nks-ui"
            self.run_id = command[command.index("--run-id") + 1]
            self.output = output
            self.stdout = kwargs["stdout"]
            self.calls = 0
            self.write_report()
        def write_report(self, **changes):
            report = {"scope": runner.SCOPE, "runID": self.run_id, "checksPassed": True,
                      "status": "checksComplete", "checks": [f"check{i}" for i in range(8)], "failures": []}
            report.update(changes)
            (self.output / "report.json").write_text(json.dumps(report))
        def wait(self, timeout):
            return 0

    def execute(name, child=Child, proof_impl=proof, signal_impl=lambda pid, value: None):
        with patch.object(runner.subprocess, "run", side_effect=proof_impl), \
                patch.object(runner.subprocess, "Popen", side_effect=child), \
                patch.object(runner.os, "killpg", side_effect=signal_impl):
            return runner.run(executable, directory / name, 30)

    success = execute("success")
    assert success["passed"] is True
    assert success["runnerSHA256"] == hashlib.sha256((ROOT / "tools/run_nks_ui_diagnostic.py").read_bytes()).hexdigest()
    assert Path(success["runExecutable"]) != executable
    assert hashlib.sha256(Path(success["runExecutable"]).read_bytes()).hexdigest() == artifact["executableSHA256"]
    assert len(created) == 1
    with patch.object(runner.subprocess, "run", side_effect=AssertionError("should preserve prior output")):
        try:
            runner.run(executable, directory / "success", 30)
            raise AssertionError("Output evidence was overwritten")
        except ValueError:
            pass
    print("PASS dedicated proof precedes vendor child; frozen resource path and unique run identity are forwarded; prior evidence preserved")

    class Fatal(Child):
        def wait(self, timeout):
            self.stdout.write(b"SUPERTRACKER_DIAGNOSTIC_FATAL_SIGNAL=11\n")
            self.stdout.flush()
            return 139
    fatal = execute("fatal", Fatal)
    assert fatal["passed"] is False and fatal["caughtFatalSignal"] is True
    assert fatal["checksPassed"] is True and fatal["processExitPassed"] is False

    class Abnormal(Child):
        def wait(self, timeout):
            return 1
    assert execute("abnormal", Abnormal)["passed"] is False

    class Partial(Child):
        def write_report(self, **changes):
            super().write_report(status="running", checks=["one partial check"])
    assert execute("partial", Partial)["passed"] is False

    class Stale(Child):
        def write_report(self, **changes):
            super().write_report(runID="old-run")
    assert execute("stale", Stale)["passed"] is False
    print("PASS complete in-child checks cannot hide a fatal/abnormal exit, partial execution or stale report")

    signals = []
    class Stuck(Child):
        def wait(self, timeout):
            self.calls += 1
            if self.calls < 3:
                raise subprocess.TimeoutExpired("fake child", timeout)
            return -9
    timed = execute("timeout", Stuck, signal_impl=lambda pid, value: signals.append((pid, value)))
    assert timed["passed"] is False
    assert signals == [(24680, signal.SIGTERM), (24680, signal.SIGKILL), (24680, signal.SIGKILL)]
    assert (directory / "timeout/nks-ui/process.log").exists()
    assert (directory / "timeout/nks-ui/report.json").exists()
    print("PASS deadline terminates the exclusive process group, escalates, and preserves logs/report")

    # Replacing the shared build output during proof cannot change the pinned
    # image used for the vendor child or its nested scanner executable.
    original_image = executable.read_bytes()
    original_artifact = (build / "nks-ui-diagnostic-artifact.json").read_bytes()
    def rebuild_during_proof(command, **kwargs):
        executable.write_bytes(b"replacement build must never launch")
        (build / "nks-ui-diagnostic-artifact.json").unlink()
        assert Path(command[0]).read_bytes() == original_image
        return proof(command, **kwargs)
    class Pinned(Child):
        def __init__(self, command, **kwargs):
            assert Path(command[0]).read_bytes() == original_image
            assert Path(command[0]) != executable
            super().__init__(command, **kwargs)
    assert execute("concurrent-rebuild", Pinned, rebuild_during_proof)["passed"] is True
    executable.write_bytes(original_image)
    (build / "nks-ui-diagnostic-artifact.json").write_bytes(original_artifact)
    print("PASS concurrent rebuild cannot replace the verified per-run executable; actual runner hash is preserved")

    for exception in (KeyboardInterrupt(), RuntimeError("wait failed")):
        signals = []
        class Interrupted(Child):
            def wait(self, timeout):
                self.calls += 1
                if self.calls == 1:
                    raise exception
                return -15
        interrupted = execute("interrupted-" + type(exception).__name__, Interrupted,
                              signal_impl=lambda pid, value: signals.append((pid, value)))
        assert interrupted["passed"] is False and interrupted["processExitCode"] == -15
        assert signals == [(24680, signal.SIGTERM), (24680, signal.SIGKILL)]
        assert any(type(exception).__name__ in item for item in interrupted["failures"])
    print("PASS interruption and unexpected child-wait errors clean the exclusive process group and preserve failure evidence")

    prior_handler = signal.getsignal(signal.SIGTERM)
    signals = []
    class Terminated(Child):
        def wait(self, timeout):
            self.calls += 1
            if self.calls == 1:
                signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)
            return -15
    terminated = execute("parent-sigterm", Terminated,
                         signal_impl=lambda pid, value: signals.append((pid, value)))
    assert terminated["passed"] is False and terminated["processExitCode"] == -15
    assert signals == [(24680, signal.SIGTERM), (24680, signal.SIGKILL)]
    assert any("SIGTERM" in item for item in terminated["failures"])
    assert signal.getsignal(signal.SIGTERM) == prior_handler
    print("PASS parent SIGTERM enters bounded cleanup, preserves failed evidence and restores its previous handler")

    old_count = len(created)
    wrong_proof = lambda command, **kwargs: subprocess.CompletedProcess(command, 0, "OLD_UNGUARDED_SMOKE\n", "")
    assert execute("wrong-proof", proof_impl=wrong_proof)["passed"] is False
    assert len(created) == old_count
    web_file = Path(artifact["webRoot"]) / "nks-controls.js"
    web_file.write_bytes(web_file.read_bytes() + b"\n// tampered\n")
    with patch.object(runner.subprocess, "run", side_effect=AssertionError("must reject before even proof")):
        rejected = runner.run(executable, directory / "modified-resource", 30)
    assert rejected["passed"] is False and len(created) == old_count
    print("PASS wrong proof and altered interface snapshot are rejected before vendor launch")

    def failed_compile(command, **kwargs):
        raise subprocess.CalledProcessError(1, command)
    with patch.object(sys, "argv", ["build_nks_ui_diagnostic.py", "--build-dir", str(build)]), \
            patch("platform.machine", return_value="arm64"), \
            patch("subprocess.check_output", return_value="/Fixture/MacOSX.sdk\n"), \
            patch("subprocess.run", side_effect=failed_compile), contextlib.redirect_stdout(io.StringIO()):
        try:
            runpy.run_path(str(ROOT / "tools/build_nks_ui_diagnostic.py"), run_name="__main__")
            raise AssertionError("Failed rebuild was accepted")
        except subprocess.CalledProcessError:
            pass
    assert not (build / "nks-ui-diagnostic-artifact.json").exists()
    print("PASS a failed rebuild invalidates prior executable authorization")

    # A frozen production tree may predate optional unit-range timing. Its own
    # diagnostic files are deliberately poisoned: tooling must come from ROOT.
    frozen = directory / "frozen-production"
    shutil.copytree(Path(artifact["sourceDirectory"]), frozen)
    for name in ["Sources/TrackerEngine/TrackerUnitRange.cpp", "Sources/TrackerEngine/include/TrackerUnitRange.h"]:
        (frozen / name).unlink()
    frozen_host = frozen / "Sources/SuperTracker/AudioHost.swift"
    frozen_host.write_bytes(frozen_host.read_bytes() + b"\n// frozen production host marker\n")
    frozen_web = frozen / "Resources/Web/app-boot.js"
    frozen_web.write_bytes(frozen_web.read_bytes() + b"\n// frozen production UI marker\n")
    (frozen / "tests/NativeNKSUIAcceptance.swift").write_text("WRONG FROZEN TOOLING MUST NOT BE USED")
    build = directory / "frozen-build"
    compiler_calls.clear()
    with patch.object(sys, "argv", ["build_nks_ui_diagnostic.py", "--build-dir", str(build), "--source-root", str(frozen)]), \
            patch("platform.machine", return_value="arm64"), \
            patch("subprocess.check_output", return_value="/Fixture/MacOSX.sdk\n"), \
            patch("subprocess.run", side_effect=compiler), contextlib.redirect_stdout(io.StringIO()):
        runpy.run_path(str(ROOT / "tools/build_nks_ui_diagnostic.py"), run_name="__main__")
    assert len(compiler_calls) == 8
    frozen_artifact = json.loads((build / "nks-ui-diagnostic-artifact.json").read_text())
    frozen_manifest = json.loads((build / "source-manifest.json").read_text())
    origins = json.loads((build / "source-provenance.json").read_text())
    assert frozen_artifact["includesTrackerUnitRange"] is False
    assert frozen_artifact["productionSourceRoot"] == str(frozen.resolve())
    assert frozen_artifact["diagnosticToolingRoot"] == str(ROOT)
    assert not any("TrackerUnitRange" in path for path in frozen_manifest)
    assert "TrackerUnitRange" not in (build / "modules/TrackerEngine/module.modulemap").read_text()
    assert frozen_manifest["Sources/SuperTracker/AudioHost.swift"] == hashlib.sha256(frozen_host.read_bytes()).hexdigest()
    assert frozen_manifest["Resources/Web/app-boot.js"] == hashlib.sha256(frozen_web.read_bytes()).hexdigest()
    assert frozen_manifest["tests/NativeNKSUIAcceptance.swift"] == hashlib.sha256((ROOT / "tests/NativeNKSUIAcceptance.swift").read_bytes()).hexdigest()
    assert origins["entries"]["Sources/SuperTracker/AudioHost.swift"]["origin"] == "production"
    assert origins["entries"]["tests/NativeNKSUIAcceptance.swift"]["origin"] == "diagnostic-tooling"
    assert origins["entries"]["tests/NativeNKSGuard.cpp"]["sourcePath"] == str(ROOT / "tests/NativeNKSGuard.cpp")
    assert runner.verify_artifact(build / frozen_artifact["executable"])[0] == frozen_artifact
    print("PASS --source-root pins frozen host/SDK/UI separately from current diagnostic tooling and omits absent unit-range pair")

    for retained in ["Sources/TrackerEngine/TrackerUnitRange.cpp", "Sources/TrackerEngine/include/TrackerUnitRange.h"]:
        half = frozen / retained
        half.parent.mkdir(parents=True, exist_ok=True)
        half.write_text("one half is not a complete source feature")
        with patch.object(sys, "argv", ["build_nks_ui_diagnostic.py", "--build-dir", str(build), "--source-root", str(frozen)]), \
                patch("subprocess.run", side_effect=AssertionError("incomplete frozen source must not compile")), \
                contextlib.redirect_stderr(io.StringIO()):
            try:
                runpy.run_path(str(ROOT / "tools/build_nks_ui_diagnostic.py"), run_name="__main__")
                raise AssertionError("Incomplete TrackerUnitRange pair was accepted")
            except SystemExit as error:
                assert error.code == 2
        assert not (build / "nks-ui-diagnostic-artifact.json").exists()
        half.unlink()
    print("PASS either missing unit-range half fails before compilation and invalidates old build proof")

    vst2_paths = ["Sources/VST2Host/VST2Host.mm", "Sources/VST2Host/include/VST2Host.h"]
    vst2_bytes = {name: (frozen / name).read_bytes() for name in vst2_paths}
    # An imported module cannot silently vanish, even when both files are absent.
    for absent in [vst2_paths[:1], vst2_paths[1:], vst2_paths]:
        for name in absent:
            (frozen / name).unlink()
        with patch.object(sys, "argv", ["build_nks_ui_diagnostic.py", "--build-dir", str(build), "--source-root", str(frozen)]), \
                patch("subprocess.run", side_effect=AssertionError("missing Swift dependency must not compile")), \
                contextlib.redirect_stderr(io.StringIO()):
            try:
                runpy.run_path(str(ROOT / "tools/build_nks_ui_diagnostic.py"), run_name="__main__")
                raise AssertionError("Missing VST2Host dependency was accepted")
            except SystemExit as error:
                assert error.code == 2
        assert not (build / "nks-ui-diagnostic-artifact.json").exists()
        for name in absent:
            (frozen / name).write_bytes(vst2_bytes[name])
    print("PASS partial VST2 module or absent module imported by Swift fails before compilation")

    # Minimal historical Swift tree has no VST2 imports; source-root builds must
    # not borrow the current repository's host module into that frozen snapshot.
    for path in (frozen / "Sources/SuperTracker").glob("*.swift"):
        path.unlink()
    (frozen / "Sources/SuperTracker/Historical.swift").write_text("import Foundation\n")
    for name in vst2_paths:
        (frozen / name).unlink()
    compiler_calls.clear()
    with patch.object(sys, "argv", ["build_nks_ui_diagnostic.py", "--build-dir", str(build), "--source-root", str(frozen)]), \
            patch("platform.machine", return_value="arm64"), \
            patch("subprocess.check_output", return_value="/Fixture/MacOSX.sdk\n"), \
            patch("subprocess.run", side_effect=compiler), contextlib.redirect_stdout(io.StringIO()):
        runpy.run_path(str(ROOT / "tools/build_nks_ui_diagnostic.py"), run_name="__main__")
    historical = json.loads((build / "nks-ui-diagnostic-artifact.json").read_text())
    assert historical["includesVST2Host"] is False
    assert not any("VST2Host" in argument for command in compiler_calls for argument in command)
    assert not any("VST2Host" in name for name in json.loads((build / "source-manifest.json").read_text()))
    print("PASS historical source without VST2 stays self-contained and does not link current host objects")
