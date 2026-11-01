#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
stage=$(mktemp -d "${TMPDIR:-/tmp}/supertracker-recording-location.XXXXXX")
trap 'rm -rf "$stage"' EXIT
swiftc Sources/SuperTracker/AudioRecordingLocation.swift tests/audio_recording_location_tests.swift -o "$stage/test"
"$stage/test" "$stage/private-output"
