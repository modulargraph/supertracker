#!/usr/bin/env python3
"""Build an isolated production-source snapshot with a diagnostic-only entrypoint.

This never runs the executable, scans installed plug-ins, or changes the app bundle.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import uuid

parser = argparse.ArgumentParser()
parser.add_argument("--build-dir", type=Path, default=Path("test-output/nks-diagnostic-build"))
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
build = args.build_dir.resolve()
if any(part.lower().endswith(".app") for part in build.parts):
    parser.error("The diagnostic build directory must be outside every application bundle.")
build.mkdir(parents=True, exist_ok=True)
executable = build / "SuperTrackerNKSDiagnostic"
artifact_manifest = build / "nks-diagnostic-artifact.json"
# A failed rebuild must not leave an older binary authorized by stale build proof.
artifact_manifest.unlink(missing_ok=True)
# A fresh tree prevents a removed SDK header from silently resolving to a leftover
# file from an earlier build. Previous snapshots stay available for inspection.
snapshot = build / "snapshots" / uuid.uuid4().hex
snapshot.mkdir(parents=True, exist_ok=False)
manifest = {}


def copy(relative):
    source = root / relative
    destination = snapshot / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    contents = source.read_bytes()
    destination.write_bytes(contents)
    manifest[str(relative)] = hashlib.sha256(contents).hexdigest()
    return destination


# AppDelegate belongs to the ordinary entry point. Its WebKit window/termination
# tests cannot link into this audio-only diagnostic and are never invoked here.
excluded_swift = {"App.swift", "NativeBridgeSaveSafetyTests.swift", "NativeUIBridgeSelfTest.swift", "NativeTerminationSaveTests.swift"}
swift = [copy(path.relative_to(root)) for path in sorted((root / "Sources/SuperTracker").glob("*.swift")) if path.name not in excluded_swift]
entry = copy(Path("tests/NativeNKSMain.swift"))
capture_tests = copy(Path("tests/NativeVST3CaptureTests.swift"))
guard = copy(Path("tests/NativeNKSGuard.cpp"))
copy(Path("tests/vst3_signal_guard.h"))
cpp = [copy(Path(path)) for path in ("Sources/TrackerEngine/TrackerEngine.cpp", "Sources/TrackerEngine/TrackerClock.cpp", "Sources/TrackerEngine/ClockTimingMap.cpp", "Sources/TrackerEngine/TrackerUnitRange.cpp", "Sources/VST3Host/VST3Host.mm", "Sources/VST3Host/VST3SDK.cpp", "Sources/VST2Host/VST2Host.mm")]
copy(Path("Sources/TrackerEngine/ClockTimingMap.hpp"))
clock_header = copy(Path("Sources/TrackerEngine/include/TrackerClock.h"))
unit_range_header = copy(Path("Sources/TrackerEngine/include/TrackerUnitRange.h"))
for module, header in (("TrackerEngine", "TrackerEngine.h"), ("VST3Host", "VST3Host.h"), ("VST2Host", "VST2Host.h")):
    header_path = copy(Path("Sources") / module / "include" / header)
    module_dir = build / "modules" / module
    module_dir.mkdir(parents=True, exist_ok=True)
    public_headers = [header_path] + ([clock_header, unit_range_header] if module == "TrackerEngine" else [])
    header_declarations = " ".join(f'header "{path}"' for path in public_headers)
    (module_dir / "module.modulemap").write_text(f'module {module} {{ {header_declarations} export * }}\n')
sdk = root / "Vendor/vst3sdk/pluginterfaces"
for path in sorted(sdk.rglob("*")):
    if path.is_file() and ".git" not in path.parts:
        copy(path.relative_to(root))
source_manifest = build / "source-manifest.json"
source_manifest.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
for relative, digest in manifest.items():
    if hashlib.sha256((root / relative).read_bytes()).hexdigest() != digest:
        raise RuntimeError(f"Source changed while the diagnostic snapshot was copied: {relative}")

architecture = platform.machine()
if architecture not in ("arm64", "x86_64"):
    parser.error(f"Unsupported diagnostic architecture: {architecture}")
target = f"{architecture}-apple-macosx14.0"
sdk_path = subprocess.check_output(["xcrun", "--sdk", "macosx", "--show-sdk-path"], text=True).strip()
cache = build / "module-cache"
cache.mkdir(exist_ok=True)
commands = []

objects = []
for source in [*cpp, guard]:
    obj = build / (source.stem + ".o")
    command = ["xcrun", "--sdk", "macosx", "clang++", "-std=c++17", "-O0", "-g",
               "-target", target, "-isysroot", sdk_path, "-DSWIFT_PACKAGE", "-DDEBUG=1",
               "-D_LIBCPP_HARDENING_MODE=_LIBCPP_HARDENING_MODE_DEBUG", "-fvisibility-inlines-hidden",
               "-fmodules", "-fno-cxx-modules", f"-fmodules-cache-path={cache / 'clang'}"]
    if source.suffix == ".mm":
        command.append("-fobjc-arc")
    command.extend(["-c", str(source), "-I", str(snapshot / "Sources/TrackerEngine/include"),
                    "-I", str(snapshot / "Sources/VST2Host/include"),
                    "-I", str(snapshot / "Vendor/vst3sdk"), "-o", str(obj)])
    commands.append(command)
    objects.append(obj)
command = ["xcrun", "--sdk", "macosx", "swiftc", "-swift-version", "5", "-DSELFTESTS", "-parse-as-library", "-g", "-Onone",
           "-target", target, "-sdk", sdk_path, "-DDEBUG", "-DSWIFT_PACKAGE", "-module-cache-path", str(cache),
           "-I", str(build / "modules/TrackerEngine"), "-I", str(build / "modules/VST3Host"),
           "-I", str(build / "modules/VST2Host"),
           *map(str, swift), str(entry), str(capture_tests), *map(str, objects), "-lc++"]
for framework in ("AppKit", "WebKit", "AVFoundation", "AudioToolbox", "CoreAudio", "CoreMIDI", "CoreFoundation", "IOKit", "IOUSBHost"):
    command.extend(["-framework", framework])
command.extend(["-o", str(executable)])
commands.append(command)
(build / "compile-commands.json").write_text(json.dumps([
    {"directory": str(root), "arguments": command} for command in commands
], indent=2) + "\n")
for command in commands:
    subprocess.run(command, cwd=root, check=True)
artifact_manifest.write_text(json.dumps({
    "schema": 1, "kind": "SuperTrackerNKSDiagnostic", "executable": executable.name,
    "executableSHA256": hashlib.sha256(executable.read_bytes()).hexdigest(),
    "sourceManifestSHA256": hashlib.sha256(source_manifest.read_bytes()).hexdigest(),
    "sourceDirectory": str(snapshot),
    "entryPoint": "tests/NativeNKSMain.swift", "fatalGuard": "tests/NativeNKSGuard.cpp",
    "productionAppEntryExcluded": "Sources/SuperTracker/App.swift" not in manifest,
    "target": target, "sdk": sdk_path
}, indent=2, sort_keys=True) + "\n")
print(executable)
