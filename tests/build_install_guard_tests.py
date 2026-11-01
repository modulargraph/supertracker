#!/usr/bin/env python3
"""scripts/build.sh refreshes /Applications/SuperTracker.app only from a release build of the main checkout.

A debug build (scripts/run.sh) or a linked git worktree's branch build must not replace the app the user runs.
Runs build.sh's own refreshes_installed_copy function in temporary repositories; nothing is built or installed."""
from pathlib import Path
import re
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
source = (root / 'scripts/build.sh').read_text()
match = re.search(r'^refreshes_installed_copy\(\) \{.*?^\}$', source, re.M | re.S)
assert match, 'build.sh defines refreshes_installed_copy'
function = match.group(0)


def git(cwd, *args):
    subprocess.run(['git', '-c', 'user.name=t', '-c', 'user.email=t@example.invalid', *args], cwd=cwd, check=True,
                   capture_output=True)


def refreshes(cwd, configuration, app=None):
    app = app or str(Path(cwd).resolve() / 'build/SuperTracker.app')
    script = function + '\nrefreshes_installed_copy "$1" "$2"'
    return subprocess.run(['bash', '-c', script, 'guard', configuration, app], cwd=cwd).returncode == 0


with tempfile.TemporaryDirectory(prefix='supertracker-build-guard-') as temporary:
    main = Path(temporary).resolve() / 'main'
    main.mkdir()
    git(main, 'init', '-q')
    git(main, 'commit', '-q', '--allow-empty', '-m', 'base')
    linked = Path(temporary).resolve() / 'linked'
    git(main, 'worktree', 'add', '-q', str(linked))
    outside = Path(temporary).resolve() / 'outside'
    outside.mkdir()

    assert refreshes(main, 'release'), 'a release build of the main checkout refreshes the installed copy'
    assert not refreshes(main, 'debug'), 'scripts/run.sh (debug) must not replace the installed release app'
    assert not refreshes(main, 'selftest'), 'a self-test build must not replace the installed app'
    assert not refreshes(main, 'release', str(main / 'build/Preview/SuperTracker.app')), 'an explicit output path never installs'
    assert not refreshes(linked, 'release'), "a linked worktree's branch build must not replace the installed app"
    assert not refreshes(outside, 'release'), 'outside a git checkout nothing is refreshed'
print('PASS build.sh refreshes /Applications only from a release build of the main checkout')
