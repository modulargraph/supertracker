#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
# Pure production waveform math, with no AVFoundation import or audio I/O.
inspection_output="${1:-$PWD/.build/test-output/sample-inspection}"
inspection_source="${2:-$PWD/Sources/SuperTracker/AudioSamples.swift}"
mkdir -p "$inspection_output/module-cache"
python3 - "$inspection_source" "$inspection_output" <<'PY'
import pathlib,sys,hashlib,json
source=pathlib.Path(sys.argv[1]);out=pathlib.Path(sys.argv[2]);raw=source.read_text()
assert raw.count('\nenum AudioSampleError:') == 1
pure=raw.split('\nenum AudioSampleError:',1)[0].replace('import AVFoundation\n','')
assert 'AVFoundation' not in pure and 'AVAudio' not in pure
(out/'DecodedSample.swift').write_text(pure)
(out/'source-manifest.json').write_text(json.dumps({'source':str(source),'sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'extractedSHA256':hashlib.sha256(pure.encode()).hexdigest(),'testSHA256':hashlib.sha256(pathlib.Path('tests/SampleInspectionBoundsTests.swift').read_bytes()).hexdigest()},indent=2)+'\n')
PY
swiftc -Onone -swift-version 5 -module-cache-path "$inspection_output/module-cache" "$inspection_output/DecodedSample.swift" tests/SampleInspectionBoundsTests.swift -o "$inspection_output/sample-inspection-tests"
"$inspection_output/sample-inspection-tests"
