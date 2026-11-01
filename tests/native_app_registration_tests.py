#!/usr/bin/env python3
"""Private-app registration cleanup tests; all process/LaunchServices calls are mocked."""
import json
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import native_regression_support as runner


class RegistrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name).resolve()
        self.output = self.root / 'evidence'
        self.output.mkdir()
        self.root_patch = patch.object(runner, 'ROOT', self.root)
        self.platform_patch = patch.object(runner.platform, 'system', return_value='Darwin')
        self.root_patch.start()
        self.platform_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.addCleanup(self.platform_patch.stop)
        self.addCleanup(self.temporary.cleanup)

    def bundle(self, relative, identifier='studio.supertracker.mac'):
        bundle = self.root / relative
        executable = bundle / 'Contents/MacOS/SuperTracker'
        executable.parent.mkdir(parents=True)
        executable.write_bytes(b'not executable: this fixture is never launched')
        (bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleIdentifier': identifier, 'CFBundleExecutable': executable.name,
        }))
        return executable

    def test_private_output_and_build_bundles_unregister_exactly(self):
        for path in ['test-output/case/QA.app', 'build/KeyboardQA/QA.app']:
            executable = self.bundle(path)
            with patch.object(runner.subprocess, 'run', return_value=Mock(returncode=0)) as unregister:
                result = runner.unregister_private_test_bundle([str(executable)], self.output)
            self.assertTrue(result['unregistered'])
            unregister.assert_called_once_with(
                [runner.LSREGISTER, '-u', str(executable.parent.parent.parent)],
                capture_output=True, text=True, timeout=5)

    def test_current_app_and_symlink_to_current_app_are_excluded(self):
        executable = self.bundle('build/SuperTracker.app')
        link = self.root / 'test-output/alias.app'
        link.parent.mkdir()
        link.symlink_to(executable.parent.parent.parent, target_is_directory=True)
        with patch.object(runner.subprocess, 'run') as unregister:
            for candidate in [executable, link / 'Contents/MacOS/SuperTracker']:
                self.assertIsNone(runner.unregister_private_test_bundle([str(candidate)], self.output))
        unregister.assert_not_called()

    def test_applications_request_is_never_unregistered(self):
        with patch.object(runner.subprocess, 'run') as unregister:
            self.assertIsNone(runner.unregister_private_test_bundle(
                ['/Applications/SuperTracker.app/Contents/MacOS/SuperTracker'], self.output))
        unregister.assert_not_called()

    def test_private_path_resolving_into_applications_is_excluded(self):
        with patch.object(Path, 'resolve', return_value=Path('/Applications/SuperTracker.app/Contents/MacOS/SuperTracker')), \
                patch.object(runner.subprocess, 'run') as unregister:
            self.assertIsNone(runner.unregister_private_test_bundle(
                [str(self.root / 'test-output/alias.app/Contents/MacOS/SuperTracker')], self.output))
        unregister.assert_not_called()

    def test_outside_bundle_and_symlink_escape_are_excluded(self):
        executable = self.bundle('outside/QA.app')
        link = self.root / 'test-output/alias.app'
        link.parent.mkdir()
        link.symlink_to(executable.parent.parent.parent, target_is_directory=True)
        with patch.object(runner.subprocess, 'run') as unregister:
            for candidate in [executable, link / 'Contents/MacOS/SuperTracker']:
                self.assertIsNone(runner.unregister_private_test_bundle([str(candidate)], self.output))
        unregister.assert_not_called()

    def test_unrelated_identifier_or_executable_are_excluded(self):
        wrong_id = self.bundle('test-output/other/QA.app', 'studio.supertrackerUnrelated')
        wrong_executable = self.bundle('test-output/helper/QA.app')
        helper = wrong_executable.with_name('Helper')
        helper.write_bytes(b'never run')
        with patch.object(runner.subprocess, 'run') as unregister:
            for candidate in [wrong_id, helper]:
                self.assertIsNone(runner.unregister_private_test_bundle([str(candidate)], self.output))
        unregister.assert_not_called()

    def test_renamed_output_directory_with_compatibility_symlink(self):
        executable = self.bundle('test-output.noindex/case/QA.app')
        (self.root / 'test-output').symlink_to(self.root / 'test-output.noindex', target_is_directory=True)
        with patch.object(runner.subprocess, 'run', return_value=Mock(returncode=0)):
            result = runner.unregister_private_test_bundle([str(executable)], self.output)
        self.assertTrue(result['unregistered'])

    def test_execute_waits_for_exit_before_unregister_and_keeps_failure(self):
        executable = self.bundle('test-output/exit/QA.app')
        events = []
        process = Mock()
        process.wait.side_effect = lambda **kwargs: events.append('exited') or 7
        def unregister(*args, **kwargs):
            events.append('unregister')
            return Mock(returncode=0)
        with patch.object(runner.subprocess, 'Popen', return_value=process), \
                patch.object(runner.subprocess, 'run', side_effect=unregister):
            result = runner.execute([str(executable)], self.output, 'exit', 1)
        self.assertEqual(events, ['exited', 'unregister'])
        self.assertEqual(result['exitCode'], 7)
        self.assertTrue(result['launchServicesCleanup']['unregistered'])
        self.assertEqual(json.loads((self.output / 'exit-status.json').read_text()), result)

    def test_timeout_cleanup_only_after_termination_confirmed(self):
        executable = self.bundle('test-output/timeout/QA.app')
        process = Mock(pid=123456)
        process.wait.side_effect = [subprocess.TimeoutExpired('fixture', 1), -15]
        with patch.object(runner.subprocess, 'Popen', return_value=process), \
                patch.object(runner.os, 'killpg') as terminate, \
                patch.object(runner.subprocess, 'run', return_value=Mock(returncode=0)) as unregister:
            result = runner.execute([str(executable)], self.output, 'timeout', 1)
        self.assertTrue(result['timedOut'])
        self.assertTrue(result['launchServicesCleanup']['unregistered'])
        terminate.assert_called_once()
        unregister.assert_called_once()

    def test_no_cleanup_when_exit_cannot_be_confirmed(self):
        executable = self.bundle('test-output/stuck/QA.app')
        process = Mock(pid=123456)
        process.wait.side_effect = subprocess.TimeoutExpired('fixture', 1)
        with patch.object(runner.subprocess, 'Popen', return_value=process), \
                patch.object(runner.os, 'killpg'), patch.object(runner.subprocess, 'run') as unregister:
            with self.assertRaises(subprocess.TimeoutExpired):
                runner.execute([str(executable)], self.output, 'stuck', 1)
        unregister.assert_not_called()

    def test_cleanup_failure_does_not_change_fixture_result(self):
        executable = self.bundle('test-output/cleanup-error/QA.app')
        process = Mock()
        process.wait.return_value = 0
        with patch.object(runner.subprocess, 'Popen', return_value=process), \
                patch.object(runner.subprocess, 'run', side_effect=subprocess.TimeoutExpired('lsregister', 5)):
            result = runner.execute([str(executable)], self.output, 'cleanup-error', 1)
        self.assertEqual(result['exitCode'], 0)
        self.assertFalse(result['timedOut'])
        self.assertFalse(result['launchServicesCleanup']['unregistered'])
        self.assertIn('error', result['launchServicesCleanup'])


if __name__ == '__main__':
    unittest.main()
