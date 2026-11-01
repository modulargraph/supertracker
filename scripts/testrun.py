#!/usr/bin/env python3
"""Parallel, cached and selective runner for SuperTracker's test suite.

scripts/test-steps.sh is the single list of test steps (plain shell, the old serial
test.sh). This runner splits it into independent jobs and runs them in parallel:

* prelude: lines before the first job marker (shared objects, module maps, the
  freeze_test_support array) run once, serially, before anything else;
* a compile (`swiftc`/`clang++ ... -o .build/test-bin/X`) plus every later line that
  runs `.build/test-bin/X` form one job;
* every other line (bash/python scripts, shell loops) is its own job, except the
  `for SCRIPT in ...` loops, which are expanded into one job per file.

Compiles are skipped when the binary exists and a stamp of the exact command plus
the SHA-256 of every input file named on the command line is unchanged.

Usage:
  scripts/test.sh                 run everything (parallel)
  scripts/test.sh --changed [REF] run only jobs whose inputs changed vs REF
                                  (default: working tree vs HEAD, plus HEAD vs master
                                  merge-base); unknown/shared changes run everything
  scripts/test.sh --only PATTERN  run jobs whose name or command contains PATTERN
  scripts/test.sh --list          list jobs
  scripts/test.sh -j N            parallel jobs (default: CPU count)
  scripts/test.sh --serial        run jobs one at a time
  scripts/test.sh --native        also run the native app self-tests (serial, last)
"""
import argparse, concurrent.futures as cf, glob, hashlib, os, re, shlex, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STEPS = os.path.join(ROOT, 'scripts', 'test-steps.sh')
LOGS = os.path.join(ROOT, '.build', 'test-logs')
STAMPS = os.path.join(ROOT, '.build', 'test-stamps')
COMPILERS = ('swiftc', 'clang++', 'clang')
SOURCE_EXT = ('.swift', '.cpp', '.mm', '.m', '.c', '.h', '.hpp', '.o', '.js', '.py', '.sh', '.json', '.modulemap')
# Files whose change can affect any job: run everything.
GLOBAL_INPUTS = ('scripts/test-steps.sh', 'scripts/testrun.py', 'scripts/test.sh', 'Package.swift')


def logical_lines(text):
    lines, buf = [], ''
    for raw in text.splitlines():
        if raw.rstrip().endswith('\\'):
            buf += raw.rstrip()[:-1] + ' '; continue
        lines.append(buf + raw); buf = ''
    if buf: lines.append(buf)
    return lines


def output_of(line):
    try: words = shlex.split(line, posix=True)
    except ValueError: return None
    if not words or os.path.basename(words[0]) not in COMPILERS: return None
    if '-c' in words: return None  # object files belong to the prelude
    if '-o' in words:
        out = words[words.index('-o') + 1]
        if out.startswith('.build/test-bin/'): return out
    return None


ARRAYS = {}  # prelude arrays such as freeze_test_support, expanded for input detection


def expand(text):
    for name, value in ARRAYS.items():
        text = text.replace('"${%s[@]}"' % name, value).replace('${%s[@]}' % name, value)
    return text


class Job:
    def __init__(self, name, lines, compile_line=None):
        self.name, self.lines, self.compile_line = name, lines, compile_line

    def inputs(self):
        text = expand(' '.join(self.lines))
        found = set()
        for word in re.findall(r'[\w./+-]+', text):
            if word.endswith(SOURCE_EXT) and os.path.exists(os.path.join(ROOT, word)): found.add(os.path.normpath(word))
        # Scripts/tests also depend on the files they mention.
        for path in list(found):
            if path.endswith(('.sh', '.py', '.js')) and (path.startswith('scripts/') or path.startswith('tests/')):
                try: body = open(os.path.join(ROOT, path), encoding='utf-8', errors='ignore').read()
                except OSError: continue
                for word in re.findall(r'[\w./+-]+', body):
                    w = word.lstrip('./')
                    if w.endswith(SOURCE_EXT) and '/' in w and os.path.exists(os.path.join(ROOT, w)): found.add(os.path.normpath(w))
                # Node tests load web modules by bare name.
                if path.endswith('.js'):
                    for name in re.findall(r"['\"`]([\w.-]+\.(?:js|css))['\"`]", body):
                        cand = os.path.join('Resources/Web', name)
                        if os.path.exists(os.path.join(ROOT, cand)): found.add(cand)
        return found


def parse():
    lines = logical_lines(open(STEPS).read())
    for line in lines:
        m = re.match(r'^\s*(\w+)=\((.*)\)\s*$', line)
        if m: ARRAYS[m.group(1)] = m.group(2)
    prelude, jobs, by_output, started = [], [], {}, False
    for line in lines:
        stripped = line.strip()
        if not started:
            if stripped.startswith(('bash ', 'python3 ', 'for ', 'if ')) or output_of(stripped) or stripped.startswith('.build/test-bin/'):
                started = True
            else:
                prelude.append(line); continue
        if not stripped or stripped.startswith('#'): continue
        if re.match(r'^(clang\+\+|clang|swiftc) ', stripped) and ' -c ' in stripped:
            prelude.append(stripped); continue  # shared object files: build once before parallel jobs
        out = output_of(stripped)
        if out:
            job = Job(os.path.basename(out), [stripped], stripped); by_output[out] = job; jobs.append(job); continue
        # Exact binary only: `.build/test-bin/midi-sustain` must not attach to the `midi` job.
        runs = next((o for o in by_output if stripped.split()[0] == o), None)
        if runs:
            by_output[runs].lines.append(stripped); continue
        m = re.match(r'for (\w+) in (\S+); do (.*); done$', stripped)
        if m:
            var, pattern, body = m.groups()
            for path in sorted(glob.glob(os.path.join(ROOT, pattern))):
                rel = os.path.relpath(path, ROOT)
                jobs.append(Job(os.path.basename(rel), [body.replace('"$' + var + '"', shlex.quote(rel)).replace('$' + var, shlex.quote(rel))]))
            continue
        if stripped.startswith('if ') or stripped in ('fi',) or stripped.startswith(('mkdir', 'native_', 'SUPERTRACKER_', './scripts/build.sh')):
            continue  # the --native block is handled separately
        # Variable assignment lines attach to the next job (e.g. export_delivery_test_output=...).
        if re.match(r'^\w+=', stripped) and jobs:
            jobs[-1].lines.append(stripped); continue
        name = os.path.basename(stripped.split()[-1] if stripped.startswith(('bash', 'python3')) else stripped.split()[0])
        jobs.append(Job(name, [stripped]))
    return prelude, jobs


def stamp_key(line):
    line = expand(line)
    h = hashlib.sha256(line.encode())
    for word in re.findall(r'[\w./+-]+', line):
        p = os.path.join(ROOT, word)
        if word.endswith(SOURCE_EXT) and os.path.isfile(p):
            h.update(word.encode()); h.update(open(p, 'rb').read())
    for header in sorted(glob.glob(os.path.join(ROOT, 'Sources/TrackerEngine/include/*.h'))):
        if 'TrackerEngine' in line: h.update(open(header, 'rb').read())
    return h.hexdigest()


def run_job(job, env, prelude_defs):
    os.makedirs(LOGS, exist_ok=True); os.makedirs(STAMPS, exist_ok=True)
    lines, stamp, key = list(job.lines), None, None
    if job.compile_line:
        out = output_of(job.compile_line)
        key = stamp_key(job.compile_line)
        stamp = os.path.join(STAMPS, os.path.basename(out) + '.stamp')
        if os.path.exists(os.path.join(ROOT, out)) and os.path.exists(stamp) and open(stamp).read() == key:
            lines = lines[1:]
    script = prelude_defs + '\n' + '\n'.join(lines) + '\n'
    log = os.path.join(LOGS, re.sub(r'[^\w.-]', '_', job.name) + '.log')
    start = time.time()
    with open(log, 'w') as handle:
        result = subprocess.run(['bash', '-euo', 'pipefail', '-c', script], cwd=ROOT, env=env, stdout=handle, stderr=subprocess.STDOUT)
    if result.returncode == 0 and stamp and job.compile_line in lines:
        open(stamp, 'w').write(key)
    return job, result.returncode, time.time() - start, log


def changed_files(ref):
    def git(*args):
        r = subprocess.run(['git', *args], cwd=ROOT, capture_output=True, text=True)
        return [l for l in r.stdout.splitlines() if l]
    files = set(git('diff', '--name-only', 'HEAD')) | set(git('ls-files', '--others', '--exclude-standard'))
    base = ref or (git('merge-base', 'HEAD', 'master') or ['HEAD'])[0]
    if ref or base != git('rev-parse', 'HEAD')[0]:
        files |= set(git('diff', '--name-only', base, 'HEAD'))
    return {os.path.normpath(f) for f in files}


def main():
    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument('--changed', nargs='?', const='', default=None)
    ap.add_argument('--only'); ap.add_argument('--list', action='store_true')
    ap.add_argument('-j', type=int, default=os.cpu_count() or 4)
    ap.add_argument('--serial', action='store_true'); ap.add_argument('--native', action='store_true')
    args = ap.parse_args()
    prelude, jobs = parse()
    if args.list:
        for j in jobs: print(j.name, '·', len(j.inputs()), 'inputs')
        return 0
    selected = jobs
    if args.only:
        selected = [j for j in jobs if args.only in j.name or any(args.only in l for l in j.lines)]
    if args.changed is not None:
        changed = changed_files(args.changed or None)
        relevant = {f for f in changed if not f.startswith(('docs/', 'test-output/', 'build/', '.build')) and not f.endswith('.md')}
        if any(f in GLOBAL_INPUTS for f in relevant):
            print('Test runner or step list changed: running everything.')
        elif not relevant:
            print('No code changes: nothing to run.'); return 0
        else:
            mapped = [j for j in jobs if j.inputs() & relevant]
            covered = set().union(*(j.inputs() for j in jobs)) if jobs else set()
            unmapped = relevant - covered
            if unmapped and any(f.startswith(('Sources/', 'scripts/', 'tests/')) for f in unmapped):
                # Sources no job names explicitly (shared or app-only files): be safe and run all.
                print('Changes without a mapped test (%s): running everything.' % ', '.join(sorted(unmapped)[:5]))
            else:
                selected = mapped
                print('%d changed files → %d of %d jobs' % (len(relevant), len(selected), len(jobs)))
    env = dict(os.environ, CLANG_MODULE_CACHE_PATH=os.path.join(ROOT, '.build', 'clang-module-cache'))
    start = time.time()
    # Prelude: run once, and keep its variable definitions for every job.
    prelude = [l for l in prelude if not l.strip().startswith('cd "$(dirname')]
    prelude_script = '\n'.join(prelude)
    r = subprocess.run(['bash', '-euo', 'pipefail', '-c', prelude_script], cwd=ROOT, env=env)
    if r.returncode: print('FAIL prelude'); return r.returncode
    defs = '\n'.join(l for l in prelude if re.match(r'^\s*(export\s+)?\w+=', l))
    workers = 1 if args.serial else max(1, args.j)
    failures, done = [], 0
    with cf.ThreadPoolExecutor(max_workers=workers) as pool:
        futures = [pool.submit(run_job, j, env, defs) for j in selected]
        for f in cf.as_completed(futures):
            job, code, secs, log = f.result(); done += 1
            if code:
                failures.append((job, log)); print('FAIL %-40s %5.1fs  log: %s' % (job.name, secs, os.path.relpath(log, ROOT)), flush=True)
            elif secs > 20:
                print('ok   %-40s %5.1fs' % (job.name, secs), flush=True)
    if args.native and not failures:
        # Run the file (not its text via -c): the script locates the repo from $0.
        r = subprocess.run(['bash', os.path.join(ROOT, 'scripts', 'test-native.sh')], cwd=ROOT, env=env)
        if r.returncode: failures.append((Job('native', []), 'native self-tests'))
    total = time.time() - start
    for job, log in failures:
        print('\n==== %s (%s)' % (job.name, os.path.relpath(log, ROOT) if os.path.exists(str(log)) else log))
        try: print(''.join(open(log).readlines()[-25:]))
        except OSError: pass
    print('\n%d jobs, %d failed, %.0fs with %d workers' % (len(selected), len(failures), total, workers))
    return 1 if failures else 0


if __name__ == '__main__':
    sys.exit(main())
