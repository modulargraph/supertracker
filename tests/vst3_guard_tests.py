#!/usr/bin/env python3
"""Only original synthetic processes: prove fatal containment never becomes PASS.

Manual check of tests/vst3_signal_guard.h (used by the diagnostic VST3 runners); not part
of scripts/test.sh because the deliberate crashes leave macOS crash reports behind.
Usage: python3 tests/vst3_guard_tests.py
"""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
results = {}
temporary = tempfile.TemporaryDirectory(prefix='supertracker-vst3-guard-')
fixture = Path(temporary.name) / 'vst3-guard-fixture'
subprocess.run(['clang++', '-std=c++17', '-O1', '-I', str(root / 'tests'), str(root / 'tests/vst3_guard_fixture.cpp'),
                '-o', str(fixture)], check=True)
for mode in ("clean", "abrt", "bus", "segv", "fault"):
    child = subprocess.run([str(fixture), mode],
                           capture_output=True, text=True, timeout=5)
    if mode == 'clean':
        assert child.returncode == 0 and 'NORMAL_DESTRUCTORS_RAN' in child.stdout
        assert 'FATAL_SIGNAL' not in child.stderr
    else:
        expected = {'abrt': {134}, 'bus': {138}, 'segv': {139}, 'fault': {138, 139}}[mode]
        assert child.returncode in expected, (mode, child.returncode, child.stderr)
        assert 'SUPERTRACKER_DIAGNOSTIC_FATAL_SIGNAL=' in child.stderr
        assert 'NORMAL_DESTRUCTORS_RAN' not in child.stdout
    results[mode] = {'exitCode': child.returncode, 'normalDestructorsRan': 'NORMAL_DESTRUCTORS_RAN' in child.stdout,
                     'fatalContained': 'SUPERTRACKER_DIAGNOSTIC_FATAL_SIGNAL=' in child.stderr,
                     'wouldPassSmoke': child.returncode == 0}
assert all(not data['wouldPassSmoke'] for mode, data in results.items() if mode != 'clean')
print('PASS diagnostic guard: normal exit runs destructors; SIGABRT/SIGBUS/SIGSEGV/protected-memory fault are contained and remain failures.')
