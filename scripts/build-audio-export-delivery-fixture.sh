#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
fixture_build="${1:-$PWD/test-output/export-delivery/build-a}"
mkdir -p "$fixture_build/module-cache"
# Freeze inputs before compiling: working-source policy edits may be independent
# of this standalone fixture, and must not create a mixed compiler invocation.
python3 - "$fixture_build" <<'PY'
from pathlib import Path
import hashlib,json,sys
root=Path.cwd(); out=Path(sys.argv[1]).resolve()
paths=['Sources/SuperTracker/AudioExportFormat.swift','Sources/SuperTracker/AudioExportDelivery.swift','tests/AudioExportDeliveryTests.swift','scripts/build-audio-export-delivery-fixture.sh']
manifest={}
for p in paths:
    data=(root/p).read_bytes(); target=out/'source'/p
    target.parent.mkdir(parents=True,exist_ok=True); target.write_bytes(data)
    manifest[p]=hashlib.sha256(data).hexdigest()
(out/'source-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
PY
# Compile only. Root serializes actual AVAudioConverter/file fixture execution.
swiftc -O -swift-version 5 -module-cache-path "$fixture_build/module-cache" \
  "$fixture_build/source/Sources/SuperTracker/AudioExportFormat.swift" "$fixture_build/source/Sources/SuperTracker/AudioExportDelivery.swift" \
  "$fixture_build/source/tests/AudioExportDeliveryTests.swift" -framework AVFoundation \
  -o "$fixture_build/AudioExportDeliveryTests"
python3 - "$fixture_build" <<'PY'
from pathlib import Path
import hashlib,json,sys
root=Path.cwd(); out=Path(sys.argv[1]).resolve()
manifest=json.loads((out/'source-manifest.json').read_text())
assert all(hashlib.sha256((out/'source'/p).read_bytes()).hexdigest()==h for p,h in manifest.items())
binary=out/'AudioExportDeliveryTests'
result={'sourceHashes':manifest,'binary':str(binary),'binarySHA256':hashlib.sha256(binary.read_bytes()).hexdigest(),'compiledOnly':True,'scope':'AVAudioConverter and files; no AVAudioEngine, devices, MIDI, installed plug-ins or UI.'}
(out/'manifest.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
PY
