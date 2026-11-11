#!/bin/sh
# Starts the MCP server of the SuperTracker built by scripts/build.sh (the project's .mcp.json runs this).
# The server talks to the running application through its control socket; open SuperTracker first.
cd "$(dirname "$0")/.." || exit 1
for candidate in build/SuperTracker.app/Contents/MacOS/SuperTracker build/supertracker/supertracker; do
  if [ -x "$candidate" ]; then exec "$candidate" --mcp; fi
done
echo "SuperTracker is not built yet. Run scripts/build.sh first." >&2
exit 69
