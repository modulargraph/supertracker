#!/usr/bin/env python3
"""Run only the original deterministic fixture; no installed plugin is loaded."""
import json
import os
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[1]
bundle = root / "test-output/Fixture.vst3"
results = {}
for mode in ("success", "failure", "deferred", "priming", "unmapped-midi", "unmapped-midi-zero", "mapped-midi", "deferred-one-frame"):
    trace = root / f"test-output/vst3-lifecycle-{mode}.log"
    trace.write_text("")
    env = dict(os.environ, ST_VST3_FIXTURE_TRACE=str(trace))
    if mode == "failure":
        env["ST_VST3_FIXTURE_FAIL_PROCESSOR"] = "1"
    if mode == "deferred":
        env["ST_VST3_FIXTURE_DEFER_CONTROLLER"] = "1"
    if mode == "priming":
        env["ST_VST3_FIXTURE_DEFER_STARTUP"] = "1"
    if mode == "unmapped-midi":
        env["ST_VST3_FIXTURE_FAILED_MIDI_OUTPUT"] = "1"
    if mode == "unmapped-midi-zero":
        env["ST_VST3_FIXTURE_FAILED_MIDI_ZERO_OUTPUT"] = "1"
    if mode == "mapped-midi":
        env["ST_VST3_FIXTURE_VALID_MIDI"] = "1"
    if mode == "deferred-one-frame":
        env["ST_VST3_FIXTURE_DEFER_ONE_FRAME_EDIT"] = "1"
    result = subprocess.run([str(root / "test-output/vst3-lifecycle-tests"), str(bundle), mode],
                            env=env, capture_output=True, text=True, timeout=15)
    lines = trace.read_text().splitlines()
    assert result.returncode == 0, result.stderr
    assert lines.count("bundle.entry") == 1
    assert lines.count("bundle.exit") == 1
    assert lines.count("component.create") == lines.count("component.destroy")
    assert lines.count("component.initialize") == lines.count("component.terminate")
    assert lines.count("component.context.shared") == lines.count("component.initialize")
    assert "component.context.different" not in lines and "connection.connect" not in lines
    assert lines.count("factory.context.valid") == 1
    assert max(i for i, line in enumerate(lines) if line == "component.destroy") < lines.index("factory.destroy")
    assert lines.index("factory.destroy") < lines.index("bundle.exit") < lines.index("static.destroy")
    results[mode] = {"passed": True, "exitCode": result.returncode, "order": lines}
    print(result.stdout.strip())
(root / "test-output/vst3-lifecycle-fixture.json").write_text(json.dumps({"passed": True, "checks": results}, indent=2) + "\n")
print("PASS plugin and factory cleanup precedes bundle exit and static destruction on success and exceptional unwind.")
