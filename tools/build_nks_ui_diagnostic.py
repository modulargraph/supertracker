#!/usr/bin/env python3
"""Build the isolated installed-NKS WebKit diagnostic; never execute it.

This never runs the executable, scans installed plug-ins, or changes the app bundle.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import re
import subprocess
import uuid

parser = argparse.ArgumentParser()
parser.add_argument("--source-root", type=Path, help="Frozen production source tree; diagnostic tooling always comes from this repository.")
parser.add_argument("--build-dir", type=Path, default=Path("test-output/nks-ui-diagnostic-build"))
args = parser.parse_args()
tooling_root = Path(__file__).resolve().parents[1]
root = args.source_root.resolve() if args.source_root else tooling_root
build = args.build_dir.resolve()
if any(part.lower().endswith(".app") for part in build.parts):
    parser.error("The diagnostic build directory must be outside every application bundle.")
build.mkdir(parents=True, exist_ok=True)
executable = build / "SuperTrackerNKSUIDiagnostic"
artifact_manifest = build / "nks-ui-diagnostic-artifact.json"
# A failed rebuild must not leave an older binary authorized by stale build proof.
artifact_manifest.unlink(missing_ok=True)
if not all((root / directory).is_dir() for directory in ("Sources/SuperTracker", "Sources/TrackerEngine", "Sources/VST3Host", "Resources/Web", "Vendor/vst3sdk/pluginterfaces")):
    parser.error("The production source root is missing required Sources, Resources/Web or VST3 SDK directories.")
# A fresh tree prevents a removed SDK header from silently resolving to a leftover
# file from an earlier build. Previous snapshots stay available for inspection.
snapshot = build / "snapshots" / uuid.uuid4().hex
snapshot.mkdir(parents=True, exist_ok=False)
manifest = {}
provenance = {}
source_locations = {}


def copy(relative, diagnostic=False):
    source = (tooling_root if diagnostic else root) / relative
    destination = snapshot / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    contents = source.read_bytes()
    destination.write_bytes(contents)
    digest = hashlib.sha256(contents).hexdigest()
    manifest[str(relative)] = digest
    source_locations[str(relative)] = source
    provenance[str(relative)] = {"origin": "diagnostic-tooling" if diagnostic else "production", "sourcePath": str(source), "sha256": digest}
    return destination


# AppDelegate belongs to the ordinary entry point. Its window/termination
# tests cannot link here. Only the explicitly selected private WebKit fixture runs.
excluded_swift = {"App.swift", "NativeBridgeSaveSafetyTests.swift", "NativeUIBridgeSelfTest.swift", "NativeTerminationSaveTests.swift"}
swift = [copy(path.relative_to(root)) for path in sorted((root / "Sources/SuperTracker").glob("*.swift")) if path.name not in excluded_swift]
entry = copy(Path("tests/NativeNKSUIMain.swift"), diagnostic=True)
acceptance_tests = copy(Path("tests/NativeNKSUIAcceptance.swift"), diagnostic=True)
# Pin the real interface alongside the native sources, without touching the app.
for resource in sorted((root / "Resources/Web").rglob("*")):
    if resource.is_file():
        copy(resource.relative_to(root))
guard = copy(Path("tests/NativeNKSGuard.cpp"), diagnostic=True)
copy(Path("tests/vst3_signal_guard.h"), diagnostic=True)
copy(Path("tools/build_nks_ui_diagnostic.py"), diagnostic=True)
copy(Path("tools/run_nks_ui_diagnostic.py"), diagnostic=True)
range_source = Path("Sources/TrackerEngine/TrackerUnitRange.cpp")
range_header = Path("Sources/TrackerEngine/include/TrackerUnitRange.h")
has_range = (root / range_source).is_file() and (root / range_header).is_file()
if ((root / range_source).exists() or (root / range_header).exists()) and not has_range:
    parser.error("TrackerUnitRange requires both its production source and header; incomplete source trees are rejected.")
cpp_paths = ["Sources/TrackerEngine/TrackerEngine.cpp", "Sources/TrackerEngine/TrackerClock.cpp", "Sources/TrackerEngine/ClockTimingMap.cpp", "Sources/VST3Host/VST3Host.mm", "Sources/VST3Host/VST3SDK.cpp"]
if has_range:
    cpp_paths.append(str(range_source))
# Older frozen production trees predate VST2. Include a complete native module
# when present, and reject partial/missing dependencies before any compilation.
vst2_source = Path("Sources/VST2Host/VST2Host.mm")
vst2_header = Path("Sources/VST2Host/include/VST2Host.h")
has_vst2 = (root / vst2_source).is_file() and (root / vst2_header).is_file()
requires_vst2 = any(re.search(r"(?m)^\s*(?:@\w+\s+)*import\s+VST2Host\b", path.read_text()) for path in swift)
if not has_vst2 and ((root / vst2_source).exists() or (root / vst2_header).exists() or requires_vst2):
    parser.error("VST2Host requires both its production source and header when present or imported by Swift.")
if has_vst2:
    cpp_paths.append(str(vst2_source))
cpp = [copy(Path(path)) for path in cpp_paths]
copy(Path("Sources/TrackerEngine/ClockTimingMap.hpp"))
clock_header = copy(Path("Sources/TrackerEngine/include/TrackerClock.h"))
unit_range_header = copy(range_header) if has_range else None
modules = [("TrackerEngine", "TrackerEngine.h"), ("VST3Host", "VST3Host.h")]
if has_vst2:
    modules.append(("VST2Host", "VST2Host.h"))
for module, header in modules:
    header_path = copy(Path("Sources") / module / "include" / header)
    module_dir = build / "modules" / module
    module_dir.mkdir(parents=True, exist_ok=True)
    public_headers = [header_path] + ([clock_header] + ([unit_range_header] if unit_range_header else []) if module == "TrackerEngine" else [])
    header_declarations = " ".join(f'header "{path}"' for path in public_headers)
    (module_dir / "module.modulemap").write_text(f'module {module} {{ {header_declarations} export * }}\n')
sdk = root / "Vendor/vst3sdk/pluginterfaces"
for path in sorted(sdk.rglob("*")):
    if path.is_file() and ".git" not in path.parts:
        copy(path.relative_to(root))
source_manifest = build / "source-manifest.json"
source_manifest.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
provenance_manifest = build / "source-provenance.json"
provenance_manifest.write_text(json.dumps({"schema": 1, "productionSourceRoot": str(root),
    "diagnosticToolingRoot": str(tooling_root), "entries": provenance}, indent=2, sort_keys=True) + "\n")
for relative, digest in manifest.items():
    if hashlib.sha256(source_locations[relative].read_bytes()).hexdigest() != digest:
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
                    *(["-I", str(snapshot / "Sources/VST2Host/include")] if has_vst2 else []),
                    "-I", str(snapshot / "Vendor/vst3sdk"), "-o", str(obj)])
    commands.append(command)
    objects.append(obj)
command = ["xcrun", "--sdk", "macosx", "swiftc", "-swift-version", "5", "-DSELFTESTS", "-parse-as-library", "-g", "-Onone",
           "-target", target, "-sdk", sdk_path, "-DDEBUG", "-DSWIFT_PACKAGE", "-module-cache-path", str(cache),
           *[argument for module, _ in modules for argument in ("-I", str(build / "modules" / module))],
           *map(str, swift), str(entry), str(acceptance_tests), *map(str, objects), "-lc++"]
for framework in ("AppKit", "WebKit", "AVFoundation", "AudioToolbox", "CoreAudio", "CoreMIDI", "CoreFoundation", "CryptoKit", "IOKit", "IOUSBHost"):
    command.extend(["-framework", framework])
command.extend(["-o", str(executable)])
commands.append(command)
(build / "compile-commands.json").write_text(json.dumps([
    {"directory": str(root), "arguments": command} for command in commands
], indent=2) + "\n")
for command in commands:
    subprocess.run(command, cwd=root, check=True)
artifact_manifest.write_text(json.dumps({
    "schema": 1, "kind": "SuperTrackerNKSUIDiagnostic", "executable": executable.name,
    "executableSHA256": hashlib.sha256(executable.read_bytes()).hexdigest(),
    "sourceManifestSHA256": hashlib.sha256(source_manifest.read_bytes()).hexdigest(),
    "sourceProvenanceSHA256": hashlib.sha256(provenance_manifest.read_bytes()).hexdigest(),
    "productionSourceRoot": str(root), "diagnosticToolingRoot": str(tooling_root), "includesTrackerUnitRange": has_range,
    "includesVST2Host": has_vst2,
    "sourceDirectory": str(snapshot), "webRoot": str(snapshot / "Resources/Web"),
    "entryPoint": "tests/NativeNKSUIMain.swift", "fatalGuard": "tests/NativeNKSGuard.cpp",
    "productionAppEntryExcluded": "Sources/SuperTracker/App.swift" not in manifest,
    "target": target, "sdk": sdk_path
}, indent=2, sort_keys=True) + "\n")
print(executable)
