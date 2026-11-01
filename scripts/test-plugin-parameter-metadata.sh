#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
stage=$(mktemp -d "${TMPDIR:-/tmp}/supertracker-parameter-metadata.XXXXXX")
trap 'rm -rf "$stage"' EXIT
swiftc Sources/SuperTracker/AudioPluginParameterMetadata.swift tests/plugin_parameter_metadata_tests.swift -framework AVFoundation -o "$stage/test"
"$stage/test" "$stage/report.json"
