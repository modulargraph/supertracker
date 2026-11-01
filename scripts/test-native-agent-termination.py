#!/usr/bin/env python3
"""Build/run hidden WebKit + Agent admission regression; synthetic native replies only.

No AudioHost, app termination, control listener, installed plug-ins, audio devices
or native file panels. Uses copied Web resources, nonpersistent WebKit storage,
and private settings. Requires macOS 14+ and a desktop session for execution.
"""
import argparse
from pathlib import Path
import sys
from native_regression_support import Build, ROOT, positive_seconds, run_modes

FIXTURE = 'agent-termination-regression-v1'


def build_fixture(output, timeout):
    build = Build('native-agent-termination', output, timeout)
    build.copy('scripts/test-native-agent-termination.py')
    build.copy('scripts/native_regression_support.py')
    swift = [build.copy(path) for path in ['Sources/SuperTracker/AgentControlProtocol.swift',
             'Sources/SuperTracker/AgentControlServer.swift', 'tests/NativeAgentTerminationTests.swift']]
    for path in sorted((ROOT / 'Resources/Web').rglob('*')):
        if path.is_file():
            build.copy(path.relative_to(ROOT))
    executable = build.directory / 'NativeAgentTermination'
    build.compile(['xcrun', 'swiftc', '-swift-version', '5', '-DSELFTESTS', '-parse-as-library', '-g', '-Onone',
                   '-target', build.target, '-sdk', build.sdk, '-module-cache-path', str(build.cache),
                   *map(str, swift), '-framework', 'AppKit', '-framework', 'WebKit', '-o', str(executable)])
    executable.chmod(0o555)
    build.finish(executable, [], FIXTURE)
    return build.directory


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build-only', action='store_true')
    group = parser.add_mutually_exclusive_group()
    group.add_argument('--output', help='New private evidence directory.')
    group.add_argument('--run-existing', help='Verify and run a previously pinned build.')
    parser.add_argument('--timeout', type=positive_seconds, default=45)
    parser.add_argument('--build-timeout', type=positive_seconds, default=600)
    args = parser.parse_args()
    if args.build_only and args.run_existing:
        parser.error('--build-only cannot be combined with --run-existing')
    directory = Path(args.run_existing).expanduser().resolve() if args.run_existing else build_fixture(args.output, args.build_timeout)
    if args.build_only:
        print('Built only; hidden WebKit fixture NOT executed. Run with --run-existing ' + str(directory))
        return 0
    modes = [('agent-termination', ['--web-root', str(directory / 'source/Resources/Web'), '--output'])]
    return 0 if run_modes(directory, FIXTURE, modes, args.timeout) else 1


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print('FAIL: ' + str(error), file=sys.stderr)
        sys.exit(1)
