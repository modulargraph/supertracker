#!/usr/bin/env python3
"""Build an isolated production-source snapshot with a diagnostic-only entrypoint.

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
parser.add_argument("--build-dir", type=Path, default=Path("test-output/routing-diagnostic-build"))
parser.add_argument("--module-cache-dir", type=Path,
                    help="Reuse a canonical compiler cache without sharing build objects or source snapshots.")
parser.add_argument("--source-root",type=Path,help="Use this frozen production source tree; diagnostic-only tests come from the repository.")
parser.add_argument("--overlay",action="append",default=[],help="Explicit production source path to copy from the working repository instead of the frozen tree.")
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
production = args.source_root.resolve() if args.source_root else root
overlays = set(args.overlay)
for relative in overlays:
    if Path(relative).is_absolute() or ".." in Path(relative).parts or not relative.startswith("Sources/"):
        parser.error("An overlay must be an explicit relative path under Sources/.")
build = args.build_dir.resolve()
if any(part.lower().endswith(".app") for part in build.parts):
    parser.error("The diagnostic build directory must be outside every application bundle.")
build.mkdir(parents=True, exist_ok=True)
executable = build / "SuperTrackerRoutingDiagnostic"
artifact_manifest = build / "routing-diagnostic-artifact.json"
# A failed rebuild must not leave an older binary authorized by stale build proof.
artifact_manifest.unlink(missing_ok=True)
# A fresh tree prevents a removed SDK header from silently resolving to a leftover
# file from an earlier build. Previous snapshots stay available for inspection.
snapshot = build / "snapshots" / uuid.uuid4().hex
snapshot.mkdir(parents=True, exist_ok=False)
manifest = {}
origins = {}


def copy(relative):
    source = (root if str(relative) in overlays or str(relative).startswith("tests/") else production) / relative
    destination = snapshot / relative
    destination.parent.mkdir(parents=True, exist_ok=True)
    contents = source.read_bytes()
    destination.write_bytes(contents)
    manifest[str(relative)] = hashlib.sha256(contents).hexdigest()
    origins[str(relative)] = str(source)
    return destination


# Compile the production audio implementation and every audio regression. The
# separate full-app build verifies App/Bridge/WebKit; none are needed to link this
# original/Apple-only runner, and excluding them keeps diagnostic iterations short.
audio_tests = {
    "NativeAudioTests.swift", "NativeAudioRecordingTests.swift", "NativeRecordingFenceTests.swift", "NativeInputAcknowledgementTests.swift", "NativeExportTransportTests.swift", "NativePluginMoveTests.swift",
    "NativePluginStateTests.swift", "NativeEndpointLifetimeTests.swift", "NativeAUEffectTests.swift",
    "NativeAUMixTimingTests.swift", "NativeModulationTests.swift", "NativeLatencyTests.swift",
    "NativePluginRecoveryTests.swift", "NativeRoutingHostTests.swift", "NativeAUStateReuseTests.swift", "NativePluginStepFXTests.swift",
    "NativeUINKSAutomationTests.swift",
}
support = {"PayloadValue.swift", "AtomicInt64.swift", "GenericAudioUnitEditor.swift", "PluginRecoveryStore.swift", "SampleSlicing.swift", "ProjectStore.swift", "ProjectMigration.swift", "KitGroups.swift", "BuiltinWavetables.swift", "WavetableFormat.swift", "SampleReferences.swift", "RenderHealth.swift",
           "HostedPluginAdapter.swift", "PluginSnapshotCapture.swift"}
swift_paths=set(path.relative_to(production) for path in (production / "Sources/SuperTracker").glob("*.swift"))
swift_paths.update(Path(relative) for relative in overlays if relative.endswith(".swift"))
swift = [copy(path) for path in sorted(swift_paths)
         if path.name.startswith(("Audio", "MIDI", "NKS", "VST3", "VST2")) or path.name in audio_tests | support]
timing_fixture_header = copy(Path("tests/vst3_timing_fixture.h"))
timing_fixture_module = build / "modules/VST3TimingFixture"
timing_fixture_module.mkdir(parents=True, exist_ok=True)
(timing_fixture_module / "module.modulemap").write_text(
    f'module VST3TimingFixture {{ header "{timing_fixture_header}" export * }}\n')
entry = copy(Path("tests/NativeRoutingMain.swift"))
guard = copy(Path("tests/NativeNKSGuard.cpp"))
copy(Path("tests/vst3_signal_guard.h"))
cpp_paths = ["Sources/TrackerEngine/TrackerEngine.cpp", "Sources/TrackerEngine/TrackerClock.cpp", "Sources/TrackerEngine/ClockTimingMap.cpp", "Sources/TrackerEngine/TrackerUnitRange.cpp", "Sources/VST3Host/VST3Host.mm", "Sources/VST3Host/VST3SDK.cpp"]
# Keep frozen pre-VST2 production snapshots self-contained, but never compile
# Swift that imports a missing or partially overlaid native host module.
vst2_source = Path("Sources/VST2Host/VST2Host.mm")
vst2_header = Path("Sources/VST2Host/include/VST2Host.h")
def production_path(relative):
    return (root if str(relative) in overlays else production) / relative
has_vst2 = production_path(vst2_source).is_file() and production_path(vst2_header).is_file()
requires_vst2 = any(re.search(r"(?m)^\s*(?:@\w+\s+)*import\s+VST2Host\b", path.read_text()) for path in swift)
if not has_vst2 and (production_path(vst2_source).exists() or production_path(vst2_header).exists() or requires_vst2):
    parser.error("VST2Host requires both its production source and header when present or imported by Swift.")
if has_vst2:
    cpp_paths.append(str(vst2_source))
cpp = [copy(Path(path)) for path in cpp_paths]
copy(Path("Sources/TrackerEngine/ClockTimingMap.hpp"))
clock_header = copy(Path("Sources/TrackerEngine/include/TrackerClock.h"))
unit_range_header = copy(Path("Sources/TrackerEngine/include/TrackerUnitRange.h"))
modules = [("TrackerEngine", "TrackerEngine.h"), ("VST3Host", "VST3Host.h")]
if has_vst2:
    modules.append(("VST2Host", "VST2Host.h"))
for module, header in modules:
    header_path = copy(Path("Sources") / module / "include" / header)
    module_dir = build / "modules" / module
    module_dir.mkdir(parents=True, exist_ok=True)
    public_headers = [header_path] + ([clock_header, unit_range_header] if module == "TrackerEngine" else [])
    header_declarations = " ".join(f'header "{path}"' for path in public_headers)
    (module_dir / "module.modulemap").write_text(f'module {module} {{ {header_declarations} export * }}\n')
sdk = production / "Vendor/vst3sdk/pluginterfaces"
for path in sorted(sdk.rglob("*")):
    if path.is_file() and ".git" not in path.parts:
        copy(path.relative_to(production))
source_manifest = build / "source-manifest.json"
source_manifest.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
for relative, digest in manifest.items():
    if hashlib.sha256(Path(origins[relative]).read_bytes()).hexdigest() != digest:
        raise RuntimeError(f"Source changed while the diagnostic snapshot was copied: {relative}")

(build / "source-origins.json").write_text(json.dumps({"productionRoot":str(production),"overlays":sorted(overlays),"origins":origins},indent=2,sort_keys=True)+"\n")
architecture = platform.machine()
if architecture not in ("arm64", "x86_64"):
    parser.error(f"Unsupported diagnostic architecture: {architecture}")
target = f"{architecture}-apple-macosx14.0"
sdk_path = subprocess.check_output(["xcrun", "--sdk", "macosx", "--show-sdk-path"], text=True).strip()
cache = args.module_cache_dir.resolve() if args.module_cache_dir else build / "module-cache"
if any(part.lower().endswith(".app") for part in cache.parts):
    parser.error("The compiler cache must be outside every application bundle.")
cache.mkdir(parents=True,exist_ok=True)
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
command = ["xcrun", "--sdk", "macosx", "swiftc", "-swift-version", "5", "-DSELFTESTS", "-parse-as-library", "-g", "-Onone", "-whole-module-optimization",
           "-target", target, "-sdk", sdk_path, "-DDEBUG", "-DSWIFT_PACKAGE", "-module-cache-path", str(cache),
           *[argument for module, _ in modules for argument in ("-I", str(build / "modules" / module))],
           "-I", str(timing_fixture_module),
           *map(str, swift), str(entry), *map(str, objects), "-lc++"]
for framework in ("AppKit", "WebKit", "AVFoundation", "AudioToolbox", "CoreAudio", "CoreMIDI", "CoreFoundation"):
    command.extend(["-framework", framework])
command.extend(["-o", str(executable)])
commands.append(command)
(build / "compile-commands.json").write_text(json.dumps([
    {"directory": str(root), "arguments": command} for command in commands
], indent=2) + "\n")
for command in commands:
    subprocess.run(command, cwd=root, check=True)
artifact_manifest.write_text(json.dumps({
    "schema": 1, "kind": "SuperTrackerRoutingDiagnostic", "executable": executable.name,
    "executableSHA256": hashlib.sha256(executable.read_bytes()).hexdigest(),
    "sourceManifestSHA256": hashlib.sha256(source_manifest.read_bytes()).hexdigest(),
    "sourceDirectory": str(snapshot),
    "includesVST2Host": has_vst2,
    "entryPoint": "tests/NativeRoutingMain.swift", "fatalGuard": "tests/NativeNKSGuard.cpp",
    "productionAppEntryExcluded": "Sources/SuperTracker/App.swift" not in manifest,
    "target": target, "sdk": sdk_path
}, indent=2, sort_keys=True) + "\n")
print(executable)
