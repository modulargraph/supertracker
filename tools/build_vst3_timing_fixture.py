#!/usr/bin/env python3
"""Compile the original VST3 timing fixture; never load or execute the bundle."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import plistlib
import subprocess
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, default=Path("test-output/vst3-timing-fixture-build"))
    parser.add_argument("--source-root", type=Path, help="SDK/VST3SDK.cpp source tree; fixture tooling always comes from this repository.")
    args = parser.parse_args()
    tooling = Path(__file__).resolve().parents[1]
    root = args.source_root.resolve() if args.source_root else tooling
    build = args.build_dir.resolve()
    if any(part.lower().endswith((".app", ".vst3")) for part in build.parts):
        parser.error("Choose a dedicated build directory outside existing application or plug-in bundles.")
    build.mkdir(parents=True, exist_ok=True)
    artifact_path = build / "vst3-timing-fixture-artifact.json"
    artifact_path.unlink(missing_ok=True)
    snapshot = build / "snapshots" / uuid.uuid4().hex
    snapshot.mkdir(parents=True)
    manifest = {}

    def copy(relative, diagnostic=False):
        origin = (tooling if diagnostic else root) / relative
        data = origin.read_bytes()
        target = snapshot / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        manifest[str(relative)] = {"sourcePath": str(origin), "origin": "fixture-tooling" if diagnostic else "sdk-source",
                                   "sha256": hashlib.sha256(data).hexdigest()}
        return target

    source = copy(Path("tests/vst3_timing_fixture.mm"), True)
    copy(Path("tests/vst3_timing_fixture.h"), True)
    copy(Path("tools/build_vst3_timing_fixture.py"), True)
    sdk_cpp = copy(Path("Sources/VST3Host/VST3SDK.cpp"))
    sdk = root / "Vendor/vst3sdk/pluginterfaces"
    if not sdk.is_dir():
        parser.error("VST3 SDK pluginterfaces directory is missing.")
    for path in sorted(sdk.rglob("*")):
        if path.is_file() and ".git" not in path.parts:
            copy(path.relative_to(root))
    for relative, entry in manifest.items():
        if hashlib.sha256(Path(entry["sourcePath"]).read_bytes()).hexdigest() != entry["sha256"]:
            raise RuntimeError("Source changed while copying fixture snapshot: " + relative)
    source_manifest = build / "source-manifest.json"
    source_manifest.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")

    architecture = platform.machine()
    if architecture not in ("arm64", "x86_64"):
        parser.error("Only macOS arm64/x86_64 fixture builds are supported.")
    target = architecture + "-apple-macosx14.0"
    sdk_path = subprocess.check_output(["xcrun", "--sdk", "macosx", "--show-sdk-path"], text=True).strip()
    bundle = snapshot / "SuperTrackerTimingProbe.vst3"
    executable = bundle / "Contents/MacOS/SuperTrackerTimingProbe"
    executable.parent.mkdir(parents=True)
    info = {"CFBundleIdentifier": "studio.supertracker.tests.timing-probe", "CFBundleExecutable": executable.name,
            "CFBundlePackageType": "BNDL", "CFBundleName": "SuperTracker Timing Probe",
            "CFBundleVersion": "1", "CFBundleShortVersionString": "1.0.0"}
    (bundle / "Contents/Info.plist").write_bytes(plistlib.dumps(info, sort_keys=True))
    command = ["xcrun", "--sdk", "macosx", "clang++", "-std=c++17", "-O2", "-g", "-fvisibility=hidden", "-bundle",
               "-target", target, "-isysroot", sdk_path, "-I", str(snapshot / "Vendor/vst3sdk"),
               str(source), str(sdk_cpp), "-framework", "CoreFoundation", "-o", str(executable)]
    (build / "compile-command.json").write_text(json.dumps({"directory": str(root), "arguments": command}, indent=2) + "\n")
    subprocess.run(command, cwd=root, check=True)
    for relative, entry in manifest.items():
        if hashlib.sha256((snapshot / relative).read_bytes()).hexdigest() != entry["sha256"]:
            raise RuntimeError("Snapshot changed during compilation: " + relative)
    artifact = {"schema": 1, "kind": "OriginalVST3TimingFixture", "classID": "53A65D54CFD64B728ECE31B97A729C04",
                "manufacturer": "SuperTracker Tests", "bundle": str(bundle), "executable": str(executable),
                "executableSHA256": hashlib.sha256(executable.read_bytes()).hexdigest(),
                "sourceManifestSHA256": hashlib.sha256(source_manifest.read_bytes()).hexdigest(),
                "snapshot": str(snapshot), "sdkSourceRoot": str(root), "fixtureToolingRoot": str(tooling),
                "snapshotVersion": 1, "snapshotBytes": 88, "capacity": 16384,
                "target": target, "sdk": sdk_path, "runtimeExecuted": False}
    artifact_path.write_text(json.dumps(artifact, indent=2, sort_keys=True) + "\n")
    print(bundle)


if __name__ == "__main__":
    main()
