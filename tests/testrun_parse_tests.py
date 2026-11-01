#!/usr/bin/env python3
"""scripts/testrun.py attaches each `.build/test-bin/X` run line to the job that builds X, never to a job whose
binary name is merely a prefix (midi / midi-sustain, store / store-boundaries, engine / engine-debug)."""
import importlib.util, os
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
spec = importlib.util.spec_from_file_location('testrun', os.path.join(ROOT, 'scripts', 'testrun.py'))
testrun = importlib.util.module_from_spec(spec); spec.loader.exec_module(testrun)
_, jobs = testrun.parse()
checks = 0
for job in jobs:
    if not job.compile_line: continue
    out = testrun.output_of(job.compile_line)
    for line in job.lines[1:]:
        first = line.split()[0]
        if first.startswith('.build/test-bin/'):
            assert first == out, f'job {job.name}: runs {first} but builds {out}'
            checks += 1
runs = {job.name: [l.split()[0] for l in job.lines[1:] if l.startswith('.build/test-bin/')] for job in jobs if job.compile_line}
for name in ('midi', 'midi-sustain', 'store', 'store-boundaries', 'engine', 'engine-debug', 'media', 'media-aliases'):
    assert runs.get(name) == ['.build/test-bin/' + name], f'{name} runs {runs.get(name)}'
    checks += 1
print(f'PASS {checks} test-runner job parsing checks')
