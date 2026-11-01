#!/usr/bin/env python3
"""Bound one native regression and preserve a process sample before terminating a hang."""
import argparse
from pathlib import Path
import shutil
import subprocess
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--timeout', type=float, default=600)
parser.add_argument('app', help='App executable')
parser.add_argument('mode', help='Native self-test flag')
parser.add_argument('output', type=Path)
args = parser.parse_args()
if args.timeout <= 0:
    parser.error('--timeout must be positive')
args.output.mkdir(parents=True, exist_ok=True)
process = subprocess.Popen([args.app, args.mode, str(args.output)])
try:
    code = process.wait(timeout=args.timeout)
except subprocess.TimeoutExpired:
    report = args.output / 'native-timeout.txt'
    print(f'FAIL native test timed out after {args.timeout:g}s; capturing {report}', file=sys.stderr, flush=True)
    sampler = shutil.which('sample')
    if sampler and process.poll() is None:
        try:
            subprocess.run([sampler, str(process.pid), '1', '-file', str(report)],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
        except (OSError, subprocess.TimeoutExpired):
            pass
    if not report.exists():
        report.write_text(f'Native test timed out: {args.mode}\nProcess sample unavailable.\n')
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
    code = 124
except KeyboardInterrupt:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
    code = 130
sys.exit(code if code >= 0 else 128 - code)
