#!/usr/bin/env python3
"""Inspect build planning with fake compilers; never build or load native vendors."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
from unittest.mock import patch

root = Path(__file__).resolve().parents[1]
script = root / "tools/build_nks_diagnostic.py"
with tempfile.TemporaryDirectory(prefix="supertracker-diagnostic-plan-") as temporary:
    build = (Path(temporary) / "build").resolve()
    calls = []

    def fake_compile(command, **kwargs):
        assert command[:3] == ["xcrun", "--sdk", "macosx"]
        assert command[3] in ("clang++", "swiftc"), "Build unexpectedly tried to execute a diagnostic"
        assert (build / "compile-commands.json").exists(), "Compiler started before command manifest was recorded"
        calls.append(command)
        Path(command[command.index("-o") + 1]).write_bytes(b"fake compiler output; not executable")
        return subprocess.CompletedProcess(command, 0)

    def invoke(compiler):
        with patch.object(sys, "argv", [str(script), "--build-dir", str(build)]), \
             patch("platform.machine", return_value="arm64"), \
             patch("subprocess.check_output", return_value="/Fixture/MacOSX.sdk\n"), \
             patch("subprocess.run", side_effect=compiler), contextlib.redirect_stdout(io.StringIO()):
            runpy.run_path(str(script), run_name="__main__")

    invoke(fake_compile)
    assert len(calls) == 9
    compiled_cpp = {Path(command[command.index("-c") + 1]).name for command in calls if command[3] == "clang++"}
    assert compiled_cpp == {"TrackerEngine.cpp", "TrackerClock.cpp", "ClockTimingMap.cpp", "TrackerUnitRange.cpp", "VST3Host.mm", "VST3SDK.cpp", "VST2Host.mm", "NativeNKSGuard.cpp"}
    manifest = json.loads((build / "compile-commands.json").read_text())
    assert [entry["arguments"] for entry in manifest] == calls
    sources = json.loads((build / "source-manifest.json").read_text())
    assert "Sources/SuperTracker/App.swift" not in sources
    assert "Sources/SuperTracker/NativeBridgeSaveSafetyTests.swift" not in sources
    assert "Sources/SuperTracker/NativeUIBridgeSelfTest.swift" not in sources
    assert "Sources/SuperTracker/NativeTerminationSaveTests.swift" not in sources
    for name in ["AudioHost.swift", "AudioPluginRack.swift", "Bridge.swift", "VST3AudioUnit.swift", "VST2AudioUnit.swift", "NativeVST3Tests.swift", "NativeVST3FixtureTests.swift"]:
        assert "Sources/SuperTracker/" + name in sources
    assert "tests/NativeNKSMain.swift" in sources and "tests/NativeNKSGuard.cpp" in sources
    assert "tests/NativeVST3CaptureTests.swift" in sources
    for name in ("TrackerEngine.h", "TrackerClock.h", "TrackerUnitRange.h"):
        assert "Sources/TrackerEngine/include/" + name in sources
        assert name in (build / "modules/TrackerEngine/module.modulemap").read_text()
    assert "Sources/VST2Host/include/VST2Host.h" in sources
    assert "VST2Host.h" in (build / "modules/VST2Host/module.modulemap").read_text()
    for command in calls:
        assert command[command.index("-target") + 1] == "arm64-apple-macosx14.0"
        if command[3] == "clang++":
            assert "-O0" in command and "-DDEBUG=1" in command and "-DSWIFT_PACKAGE" in command
            assert "-D_LIBCPP_HARDENING_MODE=_LIBCPP_HARDENING_MODE_DEBUG" in command
            assert command[command.index("-isysroot") + 1] == "/Fixture/MacOSX.sdk"
            source = Path(command[command.index("-c") + 1])
            assert ("-fobjc-arc" in command) == (source.suffix == ".mm")
            if source.name == "VST2Host.mm":
                assert str(source.parent / "include") in command
        else:
            assert command[command.index("-sdk") + 1] == "/Fixture/MacOSX.sdk"
            assert "-Onone" in command and not any(value.endswith("/App.swift") for value in command)
            assert str(build / "modules/VST2Host") in command
            assert str(build / "VST2Host.o") in command
            assert "IOKit" in command and "IOUSBHost" in command
    artifact = json.loads((build / "nks-diagnostic-artifact.json").read_text())
    executable = build / artifact["executable"]
    assert artifact["productionAppEntryExcluded"] is True
    assert artifact["executableSHA256"] == hashlib.sha256(executable.read_bytes()).hexdigest()
    assert artifact["sourceManifestSHA256"] == hashlib.sha256((build / "source-manifest.json").read_bytes()).hexdigest()

    def failed_compile(command, **kwargs):
        raise subprocess.CalledProcessError(1, command)

    try:
        invoke(failed_compile)
        raise AssertionError("A failed build was accepted")
    except subprocess.CalledProcessError:
        pass
    assert not (build / "nks-diagnostic-artifact.json").exists(), "Failed rebuild retained stale executable authorization"
print("PASS diagnostic build plan: separate entrypoint, no execution, frozen manifest, ARC/debug/SDK/target parity, precompile command manifest, artifact hashes, failed-build invalidation")
