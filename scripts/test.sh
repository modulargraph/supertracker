#!/bin/bash
# Parallel, cached test runner. See scripts/testrun.py for options (--changed, --only, --list, -j, --native).
# The test steps themselves live in scripts/test-steps.sh.
exec python3 "$(dirname "$0")/testrun.py" "$@"
