#!/usr/bin/env python3
"""Pure routing build-plan regressions; fake every compiler, never run audio."""
import contextlib
import hashlib
import io
import json
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import tempfile
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'tools/build_routing_diagnostic.py'
with tempfile.TemporaryDirectory(prefix='supertracker-routing-build-plan-') as temporary:
    directory = Path(temporary).resolve()
    build = directory / 'build'
    calls = []

    def compiler(command, **kwargs):
        assert command[:3] == ['xcrun', '--sdk', 'macosx']
        assert command[3] in ('clang++', 'swiftc'), 'Builder attempted native execution'
        assert (build / 'compile-commands.json').is_file()
        calls.append(command)
        Path(command[command.index('-o') + 1]).write_bytes(b'fake compiler output, never executable')
        return subprocess.CompletedProcess(command, 0)

    def invoke(extra=(), compile_impl=compiler):
        with patch.object(sys, 'argv', [str(SCRIPT), '--build-dir', str(build), *extra]), \
                patch('platform.machine', return_value='arm64'), \
                patch('subprocess.check_output', return_value='/Fixture/MacOSX.sdk\n'), \
                patch('subprocess.run', side_effect=compile_impl), contextlib.redirect_stdout(io.StringIO()):
            runpy.run_path(str(SCRIPT), run_name='__main__')

    invoke()
    artifact = json.loads((build / 'routing-diagnostic-artifact.json').read_text())
    manifest = json.loads((build / 'source-manifest.json').read_text())
    assert len(calls) == 9 and artifact['includesVST2Host'] is True
    required = ['Sources/VST2Host/VST2Host.mm', 'Sources/VST2Host/include/VST2Host.h',
                'Sources/SuperTracker/VST2AudioUnit.swift', 'Sources/SuperTracker/VST2Catalogue.swift',
                'Sources/SuperTracker/HostedPluginAdapter.swift', 'Sources/SuperTracker/PluginSnapshotCapture.swift',
                'Sources/SuperTracker/AudioFreezeTrackFingerprint.swift']
    assert all(path in manifest for path in required)
    assert all(path not in manifest for path in ['Sources/SuperTracker/App.swift',
               'Sources/SuperTracker/NativeTerminationSaveTests.swift', 'Sources/SuperTracker/Bridge.swift'])
    swift = calls[-1]
    assert str(build / 'modules/VST2Host') in swift and str(build / 'VST2Host.o') in swift
    assert str(build / 'modules/VST3TimingFixture') in swift
    for name in ['HostedPluginAdapter.swift', 'AudioFreezeTrackFingerprint.swift']:
        assert any(path.endswith('/' + name) for path in swift)
    vst2 = next(command for command in calls if any(path.endswith('/VST2Host.mm') for path in command))
    assert '-fobjc-arc' in vst2 and str(Path(artifact['sourceDirectory']) / 'Sources/VST2Host/include') in vst2
    assert 'VST2Host.h' in (build / 'modules/VST2Host/module.modulemap').read_text()
    assert artifact['executableSHA256'] == hashlib.sha256((build / artifact['executable']).read_bytes()).hexdigest()
    assert artifact['sourceManifestSHA256'] == hashlib.sha256((build / 'source-manifest.json').read_bytes()).hexdigest()
    print('PASS current routing build includes complete VST2 host, shared adapter, fingerprint helper, pinned commands and hashes')

    frozen = directory / 'frozen'
    shutil.copytree(Path(artifact['sourceDirectory']), frozen)
    helper = frozen / 'Sources/SuperTracker/AudioFreezeTrackFingerprint.swift'
    helper.write_text(helper.read_text() + '\n// pinned historical helper\n')
    calls.clear()
    invoke(['--source-root', str(frozen)])
    sources = json.loads((build / 'source-manifest.json').read_text())
    assert sources['Sources/SuperTracker/AudioFreezeTrackFingerprint.swift'] == hashlib.sha256(helper.read_bytes()).hexdigest()
    origins = json.loads((build / 'source-origins.json').read_text())
    assert origins['origins']['Sources/VST2Host/VST2Host.mm'] == str(frozen / 'Sources/VST2Host/VST2Host.mm')
    invoke(['--source-root', str(frozen), '--overlay', 'Sources/SuperTracker/AudioFreezeTrackFingerprint.swift'])
    sources = json.loads((build / 'source-manifest.json').read_text())
    assert sources['Sources/SuperTracker/AudioFreezeTrackFingerprint.swift'] == hashlib.sha256((ROOT / 'Sources/SuperTracker/AudioFreezeTrackFingerprint.swift').read_bytes()).hexdigest()
    print('PASS frozen production helper/VST2 origins stay pinned; only explicit overlay uses current source')

    paths = ['Sources/VST2Host/VST2Host.mm', 'Sources/VST2Host/include/VST2Host.h']
    originals = {path: (frozen / path).read_bytes() for path in paths}
    for absent in [paths[:1], paths[1:], paths]:
        for path in absent: (frozen / path).unlink()
        with contextlib.redirect_stderr(io.StringIO()):
            try:
                invoke(['--source-root', str(frozen)], lambda *a, **k: (_ for _ in ()).throw(AssertionError('missing dependency must fail before compiling')))
                raise AssertionError('Partial/imported-missing native module accepted')
            except SystemExit as error:
                assert error.code == 2
        assert not (build / 'routing-diagnostic-artifact.json').exists()
        for path in absent: (frozen / path).write_bytes(originals[path])
    print('PASS incomplete or imported-missing VST2 pair fails before compilation and removes stale artifact proof')

    for path in (frozen / 'Sources/SuperTracker').glob('*.swift'): path.unlink()
    (frozen / 'Sources/SuperTracker/AudioHistorical.swift').write_text('import Foundation\n')
    for path in paths: (frozen / path).unlink()
    calls.clear()
    invoke(['--source-root', str(frozen)])
    artifact = json.loads((build / 'routing-diagnostic-artifact.json').read_text())
    assert artifact['includesVST2Host'] is False
    assert not any('VST2Host' in argument for command in calls for argument in command)
    assert not any('VST2Host' in path for path in json.loads((build / 'source-manifest.json').read_text()))
    print('PASS pre-VST2 frozen build plan never borrows current VST2 source or module')

    def failure(command, **kwargs): raise subprocess.CalledProcessError(1, command)
    try:
        invoke(compile_impl=failure)
        raise AssertionError('Compiler failure accepted')
    except subprocess.CalledProcessError: pass
    assert not (build / 'routing-diagnostic-artifact.json').exists()
    print('PASS failed rebuild cannot retain a prior successful diagnostic artifact')
