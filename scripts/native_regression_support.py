"""Shared private build/evidence utilities for the original native fixtures.

Nothing in this module launches a fixture at import time. All child processes use
argument arrays, a private working directory, and a bounded process-group timeout.
"""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import signal
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
LSREGISTER = '/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister'


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path: Path, value) -> None:
    temporary = path.with_name(path.name + '.tmp')
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')
    temporary.replace(path)


def checked_relative(root: Path, relative: str) -> Path:
    path = (root / relative).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError('Manifest path escapes its private directory: ' + relative)
    return path


def private_environment(directory: Path) -> dict[str, str]:
    environment = dict(os.environ)
    # An inherited diagnostic override must not redirect outputs or turn on a
    # different executable mode. Keep the actual HOME and desktop identity alone.
    for key in list(environment):
        if key.startswith(('SUPERTRACKER_', 'ST_', 'DYLD_')):
            del environment[key]
    support = directory / 'support'
    temporary = directory / 'tmp'
    support.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary.mkdir(mode=0o700, parents=True, exist_ok=True)
    environment['SUPERTRACKER_SUPPORT_DIR'] = str(support)
    environment['TMPDIR'] = str(temporary) + '/'
    return environment


def unregister_private_test_bundle(command: list[str], directory: Path) -> dict | None:
    """Forget an exited private AppKit fixture, never the installed application.

    Call only after wait() confirms exit. Spotlight exclusions do not prevent
    AppKit from registering a launched test bundle with LaunchServices.
    """
    if platform.system() != 'Darwin' or not command:
        return None
    try:
        requested = Path(os.path.abspath(directory / command[0]))
        if requested.is_relative_to(Path('/Applications')):
            return None
        executable = requested.resolve(strict=True)
        if executable.is_relative_to(Path('/Applications')):
            return None
        if executable.parent.name != 'MacOS' or executable.parent.parent.name != 'Contents':
            return None
        bundle = executable.parent.parent.parent
        if bundle.suffix != '.app' or bundle == (ROOT / 'build/SuperTracker.app').resolve():
            return None
        if not any(bundle.is_relative_to((ROOT / folder).resolve()) for folder in ('test-output', 'build')):
            return None
        info_path = bundle / 'Contents/Info.plist'
        if info_path.stat().st_size > 1024 * 1024:
            return None
        with info_path.open('rb') as source:
            info = plistlib.load(source)
        identifier = info.get('CFBundleIdentifier', '')
        if not isinstance(identifier, str) or not (identifier == 'studio.supertracker' or identifier.startswith('studio.supertracker.')):
            return None
        if info.get('CFBundleExecutable') != executable.name:
            return None
    except (OSError, ValueError, TypeError, AttributeError):
        return None
    result = {'bundle': str(bundle), 'unregistered': False}
    try:
        cleanup = subprocess.run([LSREGISTER, '-u', str(bundle)], capture_output=True, text=True, timeout=5)
        result.update(unregistered=cleanup.returncode == 0, exitCode=cleanup.returncode)
        if cleanup.returncode:
            result['error'] = cleanup.stderr.strip()[:1000]
    except (OSError, subprocess.TimeoutExpired) as error:
        result['error'] = str(error)
    return result


def execute(command: list[str], directory: Path, name: str, timeout: float, environment=None) -> dict:
    directory.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    timed_out = False
    with (directory / (name + '.stdout')).open('wb') as stdout, (directory / (name + '.stderr')).open('wb') as stderr:
        process = subprocess.Popen(command, cwd=directory, env=environment, stdout=stdout, stderr=stderr, start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                code = process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                code = process.wait(timeout=3)
    result = {'command': command, 'exitCode': code, 'timedOut': timed_out,
              'seconds': round(time.monotonic() - started, 4),
              'stdout': str(directory / (name + '.stdout')), 'stderr': str(directory / (name + '.stderr'))}
    cleanup = unregister_private_test_bundle(command, directory)
    if cleanup is not None:
        result['launchServicesCleanup'] = cleanup
    write_json(directory / (name + '-status.json'), result)
    return result


def validate_report(path: Path) -> dict:
    if not path.is_file() or path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError('Missing or oversized fixture report: ' + str(path))
    report = json.loads(path.read_text())
    if not isinstance(report, dict):
        raise ValueError('Fixture report must be an object: ' + str(path))
    checks = report.get('checks')
    if report.get('passed') is not True or not isinstance(checks, list) or not checks:
        raise ValueError('Fixture report is not an explicit nonempty pass: ' + str(path))
    if any(not isinstance(check, dict) or check.get('passed') is not True for check in checks):
        raise ValueError('Fixture report contains a failing or malformed check: ' + str(path))
    return report


class Build:
    def __init__(self, label: str, output: str | None, timeout: float):
        if platform.system() != 'Darwin':
            raise RuntimeError('This fixture requires macOS 14+ and the Xcode command-line tools.')
        if output:
            self.directory = Path(output).expanduser().resolve()
            self.directory.mkdir(parents=True, exist_ok=False, mode=0o700)
        else:
            parent = ROOT / 'test-output' / label
            parent.mkdir(parents=True, exist_ok=True)
            self.directory = Path(tempfile.mkdtemp(prefix=time.strftime('%Y%m%d-%H%M%S-'), dir=parent))
        self.source = self.directory / 'source'
        self.source.mkdir()
        self.manifest = {}
        self.commands = []
        self.timeout = timeout
        self.environment = private_environment(self.directory / 'build-private')
        self.sdk = subprocess.check_output(['xcrun', '--sdk', 'macosx', '--show-sdk-path'], text=True).strip()
        self.target = platform.machine() + '-apple-macosx14.0'
        self.cache = self.directory / 'module-cache'
        self.cache.mkdir()
        print('Evidence: ' + str(self.directory), flush=True)

    def copy(self, relative: str | Path) -> Path:
        relative = str(relative)
        original = checked_relative(ROOT, relative)
        destination = checked_relative(self.source, relative)
        destination.parent.mkdir(parents=True, exist_ok=True)
        data = original.read_bytes()
        destination.write_bytes(data)
        destination.chmod(0o444)
        self.manifest[relative] = hashlib.sha256(data).hexdigest()
        return destination

    def compile(self, command: list[str]) -> None:
        index = len(self.commands)
        self.commands.append(command)
        write_json(self.directory / 'commands.json', self.commands)
        write_json(self.directory / 'source-manifest.json', self.manifest)
        print('Compile step ' + str(index + 1), flush=True)
        result = execute(command, self.directory, 'compile-' + str(index), self.timeout, self.environment)
        if result['exitCode'] != 0 or result['timedOut']:
            raise RuntimeError('Compilation failed; see ' + result['stderr'])

    def finish(self, executable: Path, extra: list[Path], fixture: str) -> dict:
        if any(digest(self.source / path) != expected for path, expected in self.manifest.items()):
            raise RuntimeError('A snapshotted source changed while compiling.')
        artifacts = {str(path.relative_to(self.directory)): digest(path) for path in [executable, *extra]}
        result = {'schema': 1, 'fixture': fixture, 'built': True, 'executed': False, 'passed': None,
                  'executable': str(executable.relative_to(self.directory)), 'sourceManifestSHA256': digest(self.directory / 'source-manifest.json'),
                  'artifacts': artifacts, 'target': self.target, 'sdk': self.sdk}
        write_json(self.directory / 'build.json', result)
        return result


def verify_build(directory: Path, fixture: str) -> dict:
    build = json.loads((directory / 'build.json').read_text())
    if build.get('schema') != 1 or build.get('fixture') != fixture or build.get('built') is not True:
        raise ValueError('Wrong or incomplete fixture build.')
    manifest_path = directory / 'source-manifest.json'
    if digest(manifest_path) != build['sourceManifestSHA256']:
        raise ValueError('Source manifest changed after compilation.')
    for relative, expected in json.loads(manifest_path.read_text()).items():
        if digest(checked_relative(directory / 'source', relative)) != expected:
            raise ValueError('Source changed after compilation: ' + relative)
    for relative, expected in build['artifacts'].items():
        if digest(checked_relative(directory, relative)) != expected:
            raise ValueError('Compiled artifact changed: ' + relative)
    checked_relative(directory, build['executable'])
    return build


def run_modes(directory: Path, fixture: str, modes: list[tuple[str, list[str]]], timeout: float) -> bool:
    build = verify_build(directory, fixture)
    run = directory / 'runs' / (time.strftime('%Y%m%d-%H%M%S-') + uuid.uuid4().hex[:8])
    run.mkdir(parents=True, mode=0o700)
    results = []
    for name, arguments in modes:
        output = run / name
        output.mkdir(mode=0o700)
        command = [str(directory / build['executable']), *arguments, str(output)]
        result = execute(command, output, 'process', timeout, private_environment(output))
        result['mode'] = name
        try:
            report = validate_report(output / 'report.json')
            result['checks'] = len(report['checks'])
            result['reportSHA256'] = digest(output / 'report.json')
            result['passed'] = result['exitCode'] == 0 and not result['timedOut']
        except (ValueError, OSError, KeyError) as error:
            result.update(passed=False, error=str(error))
        results.append(result)
        write_json(run / 'results.json', {'fixture': fixture, 'passed': all(item['passed'] for item in results), 'complete': len(results) == len(modes), 'results': results})
        print(('PASS ' if result['passed'] else 'FAIL ') + name, flush=True)
    try:
        verify_build(directory, fixture)
    except (ValueError, OSError, KeyError) as error:
        results.append({'mode': 'immutable-inputs', 'passed': False, 'error': str(error)})
    passed = all(item['passed'] for item in results)
    write_json(run / 'results.json', {'fixture': fixture, 'passed': passed, 'complete': True, 'results': results})
    print('Run report: ' + str(run / 'results.json'), flush=True)
    return passed


def positive_seconds(value: str) -> float:
    number = float(value)
    if not 1 <= number <= 3600:
        raise ValueError('Timeout must be between 1 and 3600 seconds.')
    return number
