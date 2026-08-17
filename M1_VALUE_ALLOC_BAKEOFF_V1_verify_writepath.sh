#!/usr/bin/env bash
# M1_VALUE_ALLOCATION_STRATEGY_BAKEOFF_V1 -- LIVE_WRITE_PATH_UNTOUCHED gate check.
#
# Run at EVERY phase gate. Non-zero exit means a write-path source moved, which
# is a hard-limit violation under this intent.
set -euo pipefail

WP_REPO=${WP_REPO:-/home/chris/dev/axVerity-working2}
HERE=$(cd "$(dirname "$0")" && pwd)
REC=$HERE/M1_VALUE_ALLOC_BAKEOFF_V1_writepath.p0.sha256

cd "$WP_REPO"
if grep -v '^#' "$REC" | sha256sum -c --quiet -; then
  n=$(grep -vc '^#' "$REC")
  echo "WRITE PATH UNTOUCHED: $n/$n files match writepath.p0.sha256"
else
  echo "WRITE PATH MOVED -- HARD-LIMIT VIOLATION (LIVE_WRITE_PATH_UNTOUCHED)" >&2
  exit 1
fi
