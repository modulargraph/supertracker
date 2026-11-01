#!/bin/bash
# Opt-in: scripts/probe-au-parameter-metadata.sh Digitalis 'ARP ODYSSEY'
# Each named installed AU runs in a separate process with a 40-second timeout.
set -euo pipefail
cd "$(dirname "$0")/.."
if [ "$#" -eq 0 ]; then
  echo "Usage: $0 <exact installed AU name> [<another name> ...]" >&2
  exit 2
fi
stage=$(mktemp -d "${TMPDIR:-/tmp}/supertracker-au-metadata.XXXXXX")
trap 'rm -rf "$stage"' EXIT
swiftc -module-cache-path "${TMPDIR:-/tmp}/supertracker-au-metadata-module-cache" \
  Sources/SuperTracker/AudioPluginParameterMetadata.swift tools/au_parameter_metadata_probe.swift \
  -framework AVFoundation -o "$stage/probe"
python3 - "$stage" "$@" <<'PY'
import json, pathlib, subprocess, sys
stage=pathlib.Path(sys.argv[1])
reports=[]
for index,name in enumerate(sys.argv[2:]):
    destination=stage/f'{index}.json'
    try:
        result=subprocess.run([str(stage/'probe'),name,str(destination)],capture_output=True,text=True,timeout=40)
        report=json.loads(destination.read_text()) if destination.exists() else {'requested':name,'passed':False,'error':'Probe exited without a report'}
        report['exitCode']=result.returncode
        report['passed']=bool(report.get('passed')) and result.returncode==0
        if not report['passed']:
            report['diagnostics']=(result.stdout+result.stderr)[-4000:]
    except subprocess.TimeoutExpired:
        report={'requested':name,'passed':False,'error':'Timed out after 40 seconds','exitCode':124}
    reports.append(report)
print(json.dumps({'passed':all(r['passed'] for r in reports),'plugins':reports},indent=2))
sys.exit(0 if all(r['passed'] for r in reports) else 1)
PY
