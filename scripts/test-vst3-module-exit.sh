#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Private original bundle and pinned sources. Every child exits normally so its
# actual C++/bundle destruction order remains observable; no installed plug-ins.
mkdir -p test-output
build=$(mktemp -d "$PWD/test-output/vst3-module-exit.XXXXXX")
# Remove the private build on success; keep it for debugging when a step fails.
trap 'status=$?; if [ "$status" -eq 0 ]; then rm -rf "$build"; else echo "Kept for debugging: $build" >&2; fi' EXIT
mkdir -p "$build/source/Sources/VST3Host/include" "$build/source/tests" \
  "$build/source/Vendor/vst3sdk/public.sdk/source/vst" "$build/Fixture.vst3/Contents/MacOS"
cp Sources/VST3Host/VST3Host.mm Sources/VST3Host/VST3SDK.cpp Sources/VST3Host/STHardwarePatchBaseline.h "$build/source/Sources/VST3Host/"
cp Sources/VST3Host/include/VST3Host.h "$build/source/Sources/VST3Host/include/"
cp tests/vst3_module_exit_tests.mm tests/vst3_fixture.mm "$build/source/tests/"
cp -R Vendor/vst3sdk/pluginterfaces "$build/source/Vendor/vst3sdk/"
cp -R Vendor/vst3sdk/public.sdk/source/vst/utility "$build/source/Vendor/vst3sdk/public.sdk/source/vst/"
cp scripts/test-vst3-module-exit.sh "$build/runner.sh"
python3 - "$build/Fixture.vst3/Contents/Info.plist" <<'PY'
import pathlib, plistlib, sys
pathlib.Path(sys.argv[1]).write_bytes(plistlib.dumps({
    'CFBundleIdentifier': 'studio.supertracker.tests.module-exit',
    'CFBundleExecutable': 'Fixture', 'CFBundlePackageType': 'BNDL',
    'CFBundleName': 'Original Module Exit Fixture', 'CFBundleVersion': '1',
    'CFBundleShortVersionString': '1.7.1'
}))
PY
clang++ -std=c++17 -O1 -g -fvisibility=hidden -bundle -I "$build/source/Vendor/vst3sdk" \
  "$build/source/tests/vst3_fixture.mm" "$build/source/Sources/VST3Host/VST3SDK.cpp" \
  -framework CoreFoundation -o "$build/Fixture.vst3/Contents/MacOS/Fixture"
clang++ -std=c++17 -O1 -g -fobjc-arc -I "$build/source/Vendor/vst3sdk" \
  -I "$build/source/Sources/VST3Host/include" "$build/source/Sources/VST3Host/VST3Host.mm" \
  "$build/source/tests/vst3_module_exit_tests.mm" "$build/source/Sources/VST3Host/VST3SDK.cpp" \
  -framework AppKit -framework Foundation -framework CoreFoundation -o "$build/vst3-module-exit-tests"
python3 - "$build" <<'PY'
import hashlib, json, os
from pathlib import Path
import subprocess, sys

build = Path(sys.argv[1])
results = {}
for mode, expected_exit in [('explicit-success', 0), ('explicit-error', 42), ('missing-cleanup', 0)]:
    trace = build / f'{mode}.trace.log'
    trace.write_text('')
    env = {key: value for key, value in os.environ.items() if not key.startswith('ST_VST3_FIXTURE_')}
    env['ST_VST3_FIXTURE_TRACE'] = str(trace)
    result = subprocess.run([str(build/'vst3-module-exit-tests'), str(build/'Fixture.vst3'), mode],
                            env=env, capture_output=True, text=True, timeout=20)
    (build/f'{mode}.stdout.log').write_text(result.stdout)
    (build/f'{mode}.stderr.log').write_text(result.stderr)
    lines = trace.read_text().splitlines()
    assert result.returncode == expected_exit, (mode, result.returncode, result.stdout, result.stderr, lines)
    for event in ['static.init', 'static.destroy', 'bundle.entry', 'factory.create',
                  'host.two.live.retained', 'host.one.live.retained', 'host.instances.destroyed']:
        assert lines.count(event) == 1, (mode, event, lines)
    # Without explicit cleanup the module cache is leaked at exit: bundleExit must
    # never run after the bundle's statics are destroyed (JUCE aborts there).
    for event in ['bundle.exit', 'factory.destroy', 'factory.context.valid']:
        assert lines.count(event) == (0 if mode == 'missing-cleanup' else 1), (mode, event, lines)
    for event in ['component.create', 'component.initialize', 'component.context.shared', 'component.terminate', 'component.destroy']:
        assert lines.count(event) == 2, (mode, event, lines)
    assert 'component.context.different' not in lines and 'host.unexpected.error' not in lines
    destroyed = [index for index, event in enumerate(lines) if event == 'component.destroy']
    assert lines.index('host.two.live.retained') < destroyed[0] < lines.index('host.one.live.retained') < destroyed[1]
    assert destroyed[-1] < lines.index('host.instances.destroyed')
    static = lines.index('static.destroy')
    if mode == 'missing-cleanup':
        assert lines.index('host.final.cleanup.omitted') < lines.index('host.return.success') < static, lines
        ordering = 'static.destroy only; module leaked, no bundle.exit after statics'
    else:
        factory, exit_ = [lines.index(event) for event in ['factory.destroy', 'bundle.exit']]
        assert destroyed[-1] < factory < exit_ < lines.index('host.final.cleanup') < static, lines
        ordering = 'factory.destroy < bundle.exit < static.destroy'
        if mode == 'explicit-error':
            assert lines.index('host.expected.error') < destroyed[-1] < lines.index('host.return.expected-error') < static
            assert 'EXPECTED diagnostic error:' in result.stderr
    results[mode] = {'passed': True, 'exitCode': result.returncode, 'expectedExitCode': expected_exit, 'ordering': ordering, 'trace': lines}
    print(result.stdout.strip())

sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
sources = [path for path in (build/'source').rglob('*') if path.is_file()]
report = {'passed': True, 'scope': 'Original private fixture only; normal process exits, no deliberate crash, installed vendor, GUI, device or audio rendering',
          'checks': results, 'sourceSHA256': {str(path.relative_to(build/'source')): sha(path) for path in sorted(sources)},
          'runnerSHA256': sha(build/'runner.sh'),
          'artifactSHA256': {str(path.relative_to(build)): sha(path) for path in [build/'vst3-module-exit-tests', build/'Fixture.vst3/Contents/MacOS/Fixture', build/'Fixture.vst3/Contents/Info.plist']}}
(build/'report.json').write_text(json.dumps(report, indent=2)+'\n')
print('PASS explicit cleanup preserves factory/bundle/static order on success and error; missing cleanup leaks the module instead of calling bundleExit after statics. Evidence: '+str(build/'report.json'))
PY
