#!/usr/bin/env bash
# M1_VALUE_ALLOCATION_STRATEGY_BAKEOFF_V1 -- T4 small-list cost, T5 write-path
# list_append cost. 2/3/4/8 elements, in M1, through the real bridge.
#
# The n4 cell is the write path's actual operating point:
# list_append(list_of_3(a,b,c), d), live at wp_mem_controller_step.m1:54,:68 and
# mem_controller.m1:43. Per the intent, a regression here is a HEADLINE, not a
# footnote.
#
# Usage:  ./M1_VALUE_ALLOC_BAKEOFF_V1_smalllist.sh <candidate-label> [--build]
# Env:    FEATURES, ITERS (default 1000000), REPS (default 5)
set -euo pipefail

CAND=${1:?usage: $0 <candidate-label> [--build]}
DO_BUILD=${2:-}

HERE=$(cd "$(dirname "$0")" && pwd)
WP_REPO=${WP_REPO:-/home/chris/dev/axVerity-working2}
PROBE_DIR=$WP_REPO/experiments/listalloc-m1
OUT=$HERE/bakeoff-results
BINS=$OUT/bin-$CAND
FEATURES=${FEATURES:-}
ITERS=${ITERS:-1000000}
REPS=${REPS:-5}

mkdir -p "$OUT" "$BINS"
CSV=$OUT/smalllist.csv
[ -f "$CSV" ] || echo "candidate,features,iters,rep,cell,total_ns,net_ns_per_op,peak_rss_kb" > "$CSV"

"$HERE/M1_VALUE_ALLOC_BAKEOFF_V1_verify_writepath.sh"

if [ "$DO_BUILD" = "--build" ]; then
  echo "[build] cargo build --release ${FEATURES:+--features $FEATURES}"
  ( cd "$HERE" && cargo build --release ${FEATURES:+--features "$FEATURES"} 2>&1 | tail -1 )
  echo "[eject] $PROBE_DIR/build.sh"
  "$PROBE_DIR/build.sh" >"$OUT/build-smalllist-$CAND.log" 2>&1 \
    || { echo "EJECT FAILED -- see $OUT/build-smalllist-$CAND.log" >&2; tail -40 "$OUT/build-smalllist-$CAND.log" >&2; exit 1; }
  cp "$PROBE_DIR"/build/listalloc-probe "$BINS"/
  echo "[saved] $BINS/listalloc-probe"
fi

BIN=$BINS/listalloc-probe
[ -x "$BIN" ] || { echo "not built for $CAND: $BIN" >&2; exit 1; }

med() { printf '%s\n' "$@" | sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}'; }

declare -A acc
for r in $(seq 1 "$REPS"); do
  o=$(python3 "$HERE/M1_VALUE_ALLOC_BAKEOFF_V1_runone.py" "$BIN" "$ITERS")
  rss=$(sed -n 's/^__rss_kb=//p' <<<"$o")
  null=$(sed -n 's/^null_ns=//p' <<<"$o")
  for cell in n2 n3 n4 n8; do
    tot=$(sed -n "s/^${cell}_ns=//p" <<<"$o")
    net=$(awk -v t="$tot" -v z="$null" -v i="$ITERS" 'BEGIN{printf "%.2f", (t-z)/i}')
    echo "$CAND,${FEATURES:-none},$ITERS,$r,$cell,$tot,$net,$rss" >> "$CSV"
    acc[$cell]="${acc[$cell]:-} $net"
  done
  echo "$CAND,${FEATURES:-none},$ITERS,$r,null,$null,0,$rss" >> "$CSV"
  acc[null]="${acc[null]:-} $(awk -v z="$null" -v i="$ITERS" 'BEGIN{printf "%.2f", z/i}')"
done

echo
printf '%-14s %-8s %16s %14s\n' CANDIDATE CELL "net ns/op" "elements"
printf '%-14s %-8s %16s %14s\n' "$CAND" null "$(med ${acc[null]})" "0 (loop only)"
printf '%-14s %-8s %16s %14s\n' "$CAND" n2 "$(med ${acc[n2]})" 2
printf '%-14s %-8s %16s %14s\n' "$CAND" n3 "$(med ${acc[n3]})" 3
printf '%-14s %-8s %16s %14s\n' "$CAND" n4 "$(med ${acc[n4]})" "4  <-- WRITE PATH"
printf '%-14s %-8s %16s %14s\n' "$CAND" n8 "$(med ${acc[n8]})" 8
echo
echo "appended to $CSV"
