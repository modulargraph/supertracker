#!/usr/bin/env python3
"""Build/run the original host regression fixtures without opening SuperTracker.

Usage: scripts/test-native-host-regressions.py [--build-only] [--output NEW_DIR]
       scripts/test-native-host-regressions.py --run-existing PRIVATE_BUILD

Runs hosting, synthetic MIDI, and frozen checkpoint modes sequentially. Every host
uses manual offline audio and empty MIDI source lists; the sole loaded plug-ins
are generated here from tests/vst2_debug_fixture.cpp. No catalogue scan is called.
Build-only never runs native fixtures. Evidence and module caches stay private;
source snapshots, commands, binaries, stdout/stderr, deadlines and reports persist.
"""
import argparse
import json
from pathlib import Path
import plistlib
import sys
from native_regression_support import Build, ROOT, positive_seconds, run_modes

FIXTURE = 'original-host-regressions-v1'


def build_fixture(output, timeout):
    build = Build('native-host-regressions', output, timeout)
    build.copy('scripts/test-native-host-regressions.py')
    build.copy('scripts/native_regression_support.py')
    support = {'PayloadValue.swift', 'AtomicInt64.swift', 'GenericAudioUnitEditor.swift', 'ProjectStore.swift', 'ProjectMigration.swift', 'KitGroups.swift', 'BuiltinWavetables.swift', 'WavetableFormat.swift', 'SampleReferences.swift', 'PluginRecoveryStore.swift', 'SampleSlicing.swift',
               'RenderHealth.swift', 'HostedPluginAdapter.swift', 'PluginSnapshotCapture.swift'}
    swift = [build.copy(path.relative_to(ROOT)) for path in sorted((ROOT / 'Sources/SuperTracker').glob('*.swift'))
             if path.name.startswith(('Audio', 'MIDI', 'NKS', 'VST3', 'VST2')) or path.name in support]
    swift += [build.copy('tests/' + name + '.swift') for name in ['NativeHostingDebugTests',
              'NativeMIDIPerformanceDebugTests', 'NativeFreezeCheckpointDebugTests', 'NativeHostingDebugMain']]
    sources = ['Sources/TrackerEngine/' + name + '.cpp' for name in ['TrackerEngine', 'TrackerClock', 'ClockTimingMap', 'TrackerUnitRange']]
    sources += ['Sources/VST3Host/VST3Host.mm', 'Sources/VST3Host/VST3SDK.cpp', 'Sources/VST2Host/VST2Host.mm']
    cpp = [build.copy(path) for path in sources]
    build.copy('Sources/TrackerEngine/ClockTimingMap.hpp')
    modules = ['TrackerEngine', 'VST3Host', 'VST2Host']
    for module in modules:
        headers = [build.copy(path.relative_to(ROOT)) for path in sorted((ROOT / 'Sources' / module / 'include').glob('*.h'))]
        directory = build.directory / 'modules' / module
        directory.mkdir(parents=True)
        (directory / 'module.modulemap').write_text('module ' + module + ' { ' + ' '.join('header ' + json.dumps(str(path)) for path in headers) + ' export * }\n')
    for path in sorted((ROOT / 'Vendor/vst3sdk/pluginterfaces').rglob('*')):
        if path.is_file() and '.git' not in path.parts:
            build.copy(path.relative_to(ROOT))
    original = build.copy('tests/vst2_debug_fixture.cpp')
    artifacts = []
    for suffix, flag in [('', []), ('Effect', ['-DORIGINAL_EFFECT'])]:
        bundle = build.directory / ('DebugVST2' + suffix + '.vst')
        binary = bundle / 'Contents/MacOS/DebugVST2'
        binary.parent.mkdir(parents=True)
        info = bundle / 'Contents/Info.plist'
        info.write_bytes(plistlib.dumps({'CFBundleExecutable': 'DebugVST2',
            'CFBundleIdentifier': 'studio.supertracker.tests.debug-vst2' + ('-effect' if suffix else ''),
            'CFBundleName': 'Original Debug VST2', 'CFBundlePackageType': 'BNDL',
            'CFBundleVersion': '1', 'CFBundleShortVersionString': '1'}))
        build.compile(['xcrun', 'clang++', '-std=c++17', '-O0', '-g', '-bundle', '-target', build.target,
                       '-isysroot', build.sdk, *flag, str(original), '-o', str(binary)])
        binary.chmod(0o555)
        info.chmod(0o444)
        artifacts += [binary, info]
    objects = []
    for source in cpp:
        obj = build.directory / (source.stem + '.o')
        command = ['xcrun', 'clang++', '-std=c++17', '-O0', '-g', '-target', build.target, '-isysroot', build.sdk,
                   '-DSWIFT_PACKAGE', '-fvisibility-inlines-hidden', '-fmodules', '-fno-cxx-modules',
                   '-fmodules-cache-path=' + str(build.cache / 'clang')]
        if source.suffix == '.mm':
            command += ['-fobjc-arc']
        command += ['-c', str(source), '-I', str(build.source / 'Sources/TrackerEngine/include'),
                    '-I', str(build.source / 'Sources/VST2Host/include'), '-I', str(build.source / 'Vendor/vst3sdk'), '-o', str(obj)]
        build.compile(command)
        objects.append(obj)
    executable = build.directory / 'NativeHostingDebug'
    command = ['xcrun', 'swiftc', '-swift-version', '5', '-DSELFTESTS', '-parse-as-library', '-g', '-Onone', '-whole-module-optimization',
               '-target', build.target, '-sdk', build.sdk, '-DSWIFT_PACKAGE', '-module-cache-path', str(build.cache)]
    for module in modules:
        command += ['-I', str(build.directory / 'modules' / module)]
    command += [str(path) for path in [*swift, *objects]] + ['-lc++']
    for framework in ['AppKit', 'AVFoundation', 'AudioToolbox', 'CoreAudio', 'CoreMIDI', 'CoreFoundation', 'IOKit']:
        command += ['-framework', framework]
    build.compile(command + ['-o', str(executable)])
    executable.chmod(0o555)
    build.finish(executable, artifacts, FIXTURE)
    return build.directory


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--build-only', action='store_true', help='Compile and pin artifacts without native execution.')
    group = parser.add_mutually_exclusive_group()
    group.add_argument('--output', help='New, nonexistent private evidence directory; otherwise create one under test-output.')
    group.add_argument('--run-existing', help='Run a previously pinned build after verifying every source and artifact hash.')
    parser.add_argument('--timeout', type=positive_seconds, default=120, help='Seconds per native mode (default 120).')
    parser.add_argument('--build-timeout', type=positive_seconds, default=600, help='Seconds per compiler invocation (default 600).')
    args = parser.parse_args()
    if args.build_only and args.run_existing:
        parser.error('--build-only cannot be combined with --run-existing')
    directory = Path(args.run_existing).expanduser().resolve() if args.run_existing else build_fixture(args.output, args.build_timeout)
    if args.build_only:
        print('Built only; native tests NOT executed. Run with --run-existing ' + str(directory))
        return 0
    modes = [('hosting', [str(directory / 'DebugVST2.vst')]),
             ('midi-performance', ['--midi-performance']), ('freeze-checkpoint', ['--freeze-checkpoint'])]
    return 0 if run_modes(directory, FIXTURE, modes, args.timeout) else 1


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print('FAIL: ' + str(error), file=sys.stderr)
        sys.exit(1)
