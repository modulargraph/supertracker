#!/usr/bin/env python3
"""Build immutable engine snapshots and compare offline DSP; never opens an audio device."""
import argparse
import hashlib
import json
import platform
import shutil
import statistics
import subprocess
from pathlib import Path

SCENARIOS = ['saw-16', 'saw-64', 'saw-256', 'loop-64', 'loop-256', 'saw-mod-64',
             'saw-mod-256', 'granular-mod-64', 'wavetable-mod-64', 'slice-mod-64',
             'mapped-host-16', 'mapped-host-loop-64', 'automated-region-lfo-64']


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def command(args):
    return subprocess.check_output(args, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-root', type=Path, required=True,
                        help='Checkout/snapshot containing Sources/TrackerEngine before the change')
    parser.add_argument('--output', type=Path, required=True, help='New evidence directory (must not exist)')
    parser.add_argument('--repetitions', type=int, default=5)
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error('repetitions must be positive')
    root = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    compiler = shutil.which('clang++')
    if not compiler:
        raise RuntimeError('clang++ is required')
    metadata = {'compiler': command([compiler, '--version']), 'architecture': platform.machine(),
                'system': platform.system(), 'repetitions': args.repetitions,
                'sample_rate': 48000, 'frames': 96000, 'block_frames': 256,
                'qos': 'user-initiated' if platform.system() == 'Darwin' else 'inherited',
                'limitations': 'Concurrent load, thermal state and core scheduling affect timing. '
                'CPU medians and median block p95 are descriptive, not real-time guarantees. '
                'No plugin, device, disk-streaming or GUI cost is included.'}
    if platform.system() == 'Darwin':
        metadata['cpu'] = command(['sysctl', '-n', 'machdep.cpu.brand_string'])
        metadata['cpu_count'] = int(command(['sysctl', '-n', 'hw.ncpu']))
        metadata['memory_bytes'] = int(command(['sysctl', '-n', 'hw.memsize']))
    binaries = {}
    for label, source_root in [('baseline', args.baseline_root.resolve()), ('current', root)]:
        destination = output / label
        source = destination / 'Sources/TrackerEngine'
        shutil.copytree(source_root / 'Sources/TrackerEngine', source)
        harness = destination / 'engine_dsp_benchmark.cpp'
        shutil.copyfile(root / 'tests/engine_dsp_benchmark.cpp', harness)
        binary = destination / 'benchmark'
        cmd = [compiler, '-std=c++17', '-O2', '-I', str(source / 'include'),
               str(source / 'TrackerEngine.cpp'), str(source / 'TrackerClock.cpp'),
               str(source / 'ClockTimingMap.cpp'), str(harness), '-o', str(binary)]
        subprocess.run(cmd, check=True)
        metadata[label] = {'compile_command': cmd, 'binary_sha256': sha(binary),
                           'source_sha256': {str(p.relative_to(destination)): sha(p)
                                             for p in sorted(destination.rglob('*')) if p.is_file()}}
        binaries[label] = binary
    (output / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
    rows = []
    with (output / 'measurements.jsonl').open('w') as log:
        for scenario in SCENARIOS:
            for pair in range(args.repetitions):
                for label in (['baseline', 'current'] if pair % 2 == 0 else ['current', 'baseline']):
                    result = subprocess.run([str(binaries[label]), '1', scenario],
                                            capture_output=True, text=True, timeout=120, check=True)
                    row = json.loads(result.stdout)
                    row.update(pair=pair, variant=label)
                    rows.append(row)
                    log.write(json.dumps(row) + '\n')
                    log.flush()
    summary = []
    for scenario in SCENARIOS:
        samples = [row for row in rows if row['scenario'] == scenario]
        exact = all(len({row[key] for row in samples}) == 1 for key in ['pcm', 'clock'])
        entry = {'scenario': scenario, 'pcm_and_clock_exact': exact,
                 'zero_render_allocations': all(row['render_allocations'] == 0 for row in samples)}
        for label in binaries:
            selected = [row for row in samples if row['variant'] == label]
            entry[label] = {key: statistics.median(row[key] for row in selected)
                            for key in ['cpu_ms', 'wall_ms', 'block_median_us', 'block_p95_us']}
        entry['cpu_reduction_percent'] = 100 * (1 - entry['current']['cpu_ms'] / entry['baseline']['cpu_ms'])
        summary.append(entry)
    (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    if not all(row['pcm_and_clock_exact'] and row['zero_render_allocations'] for row in summary):
        raise RuntimeError('PCM/clock equivalence or render-allocation regression; inspect summary.json')
    print(f'{len(SCENARIOS)} scenes preserve exact PCM/clock checksums with zero render allocations. {output}')


if __name__ == '__main__':
    main()
