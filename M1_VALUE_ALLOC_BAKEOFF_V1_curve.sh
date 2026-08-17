#!/usr/bin/env bash
# M1_VALUE_ALLOCATION_STRATEGY_BAKEOFF_V1 -- T1/T2 fold-cost + peak-RSS curve.
#
# MEASURE_IN_THE_SHIPPING_SUBSTRATE: every number here comes from an M1 program
# ejected by the real bridge and linked against the real runtime rlib. No Rust
# microbenchmark stands in for an M1 cost.
#
# UNCHANGED_M1_SOURCE: this script never edits the probe's .m1 files. It builds
# whatever is in experiments/blockid-m1/{lib,src} as-is. The candidate is
# selected purely by which cargo features the bridge rlib was built with.
#
# Usage:
#   ./M1_VALUE_ALLOC_BAKEOFF_V1_curve.sh <candidate-label> [--build]
#
#   <candidate-label>   e.g. C0, C1 -- names the output rows and the saved bins
#   --build             rebuild the bridge + re-eject the probes first
#
# Env:
#   FEATURES   cargo --features string for this candidate (default: none)
#   POPS       populations to sweep      (default: 1000 10000 100000)
#   REPS       reps per population       (default: 3)
#   BIGREPS    reps once pop >= 100000   (default: 1)
#   PROBES     which probes to run       (default: all four)
#
# Emits/appends: bakeoff-results/curve.csv  (one row per probe per rep)
set -euo pipefail

CAND=${1:?usage: $0 <candidate-label> [--build]}
DO_BUILD=${2:-}

HERE=$(cd "$(dirname "$0")" && pwd)
WP_REPO=${WP_REPO:-/home/chris/dev/axVerity-working2}
PROBE_DIR=$WP_REPO/experiments/blockid-m1
OUT=$HERE/bakeoff-results
BINS=$OUT/bin-$CAND
SCRATCH=${SCRATCH:-/home/chris/dev/axVerity-working2/.axverity-store/blockid-m1/bakeoff-scratch}

FEATURES=${FEATURES:-}
POPS=${POPS:-"1000 10000 100000"}
REPS=${REPS:-3}
BIGREPS=${BIGREPS:-1}
PROBES=${PROBES:-"blockid-probe listonly-probe resolve-fe-probe nullloop-probe"}

mkdir -p "$OUT" "$BINS"
CSV=$OUT/curve.csv
[ -f "$CSV" ] || echo "candidate,features,probe,population,rep,entries,fs_list_dir_ns,fold_ns,next_id,peak_rss_kb,wall_s,status" > "$CSV"

# ── LIVE_WRITE_PATH_UNTOUCHED gate, every invocation ──
"$HERE/M1_VALUE_ALLOC_BAKEOFF_V1_verify_writepath.sh"

if [ "$DO_BUILD" = "--build" ]; then
  echo "[build] cargo build --release ${FEATURES:+--features $FEATURES}"
  ( cd "$HERE" && cargo build --release ${FEATURES:+--features "$FEATURES"} 2>&1 | grep -E "^(error|warning: unused)" | head -20 || true )
  ( cd "$HERE" && cargo build --release ${FEATURES:+--features "$FEATURES"} 2>&1 | tail -1 )
  echo "[eject] $PROBE_DIR/build.sh"
  "$PROBE_DIR/build.sh" >"$OUT/build-$CAND.log" 2>&1 || { echo "EJECT FAILED -- see $OUT/build-$CAND.log" >&2; tail -30 "$OUT/build-$CAND.log" >&2; exit 1; }
  # Freeze this candidate's binaries so it can be re-measured without a rebuild.
  cp "$PROBE_DIR"/build/*probe "$BINS"/ 2>/dev/null || true
  echo "[saved] $BINS"
fi

populate() {
  local d="$1" n="$2"
  rm -rf "$d"; mkdir -p "$d"
  python3 -c "
import os,sys
d,n=sys.argv[1],int(sys.argv[2])
for i in range(n):
    open(os.path.join(d,'block-%d.bin'%i),'w').close()
" "$d" "$n"
}

med() { printf '%s\n' "$@" | sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}'; }

printf '\n%-16s %-18s %10s %14s %16s %12s\n' CANDIDATE PROBE POP "fold ns/entry" "fold ns total" "peakRSS MB"

for pop in $POPS; do
  populate "$SCRATCH" "$pop"
  reps=$REPS; [ "$pop" -ge 100000 ] && reps=$BIGREPS

  for probe in $PROBES; do
    bin=$BINS/$probe
    [ -x "$bin" ] || { echo "  (skip $probe -- not built for $CAND)"; continue; }
    # nullloop takes an iteration count; the rest take the directory.
    arg=$SCRATCH; [ "$probe" = "nullloop-probe" ] && arg=$pop

    folds=(); rsss=()
    for r in $(seq 1 "$reps"); do
      tf=$(mktemp)
      python3 "$HERE/M1_VALUE_ALLOC_BAKEOFF_V1_runone.py" "$bin" "$arg" >"$tf.out" 2>"$tf.err" || true
      o=$(cat "$tf.out")
      rss=$(sed -n 's/^__rss_kb=//p'  <<<"$o")
      wall=$(sed -n 's/^__wall_s=//p' <<<"$o")
      st=$(sed -n 's/^__status=//p'   <<<"$o"); st=${st:-999}
      e=$(sed -n 's/^entries=//p' <<<"$o"); l=$(sed -n 's/^fs_list_dir_ns=//p' <<<"$o")
      t=$(sed -n 's/^resolve_total_ns=//p' <<<"$o"); x=$(sed -n 's/^next_id=//p' <<<"$o")
      # nullloop / listonly print "n=<n> ns=<ns>" instead
      if [ -z "$t" ]; then
        e=$(sed -n 's/^n=\([0-9]*\) .*/\1/p' <<<"$o")
        t=$(sed -n 's/.* ns=//p' <<<"$o"); l=0; x=""
      fi
      # NO_SEMANTIC_CHANGE gate. The resolver folds every entry to max+1, so for
      # a directory of block-0..block-(pop-1) the only correct answer is `pop`.
      # A candidate that changes the representation and gets this wrong is a
      # STOP, not a slow result -- so it fails loudly here rather than being
      # averaged into a curve.
      if [ -n "$x" ] && [ "$x" != "$pop" ]; then
        echo "SEMANTIC DIVERGENCE: $probe pop=$pop rep=$r next_id=$x expected $pop" >&2
        echo "$CAND,${FEATURES:-none},$probe,$pop,$r,${e:-},${l:-},,${x:-},${rss:-},${wall:-},SEMANTIC_DIVERGENCE" >> "$CSV"
        exit 3
      fi
      if [ -n "$e" ] && [ "$probe" != "nullloop-probe" ] && [ "$e" != "$pop" ]; then
        echo "ENTRY COUNT MISMATCH: $probe pop=$pop rep=$r entries=$e" >&2
        exit 3
      fi
      fold=""; [ -n "$t" ] && [ -n "$l" ] && fold=$((t - l))
      echo "$CAND,${FEATURES:-none},$probe,$pop,$r,${e:-},${l:-},${fold:-},${x:-},${rss:-},${wall:-},$st" >> "$CSV"
      [ -n "$fold" ] && { folds+=("$fold"); rsss+=("$rss"); }
      [ "$st" != "0" ] && { echo "  !! $probe pop=$pop rep=$r exited $st"; sed -n '1,5p' "$tf.err"; }
      rm -f "$tf" "$tf.out" "$tf.err"
    done

    if [ "${#folds[@]}" -gt 0 ]; then
      mf=$(med "${folds[@]}"); mr=$(med "${rsss[@]}")
      printf '%-16s %-18s %10s %14s %16s %12s\n' "$CAND" "$probe" "$pop" \
        "$(awk -v a="$mf" -v b="$pop" 'BEGIN{printf "%.1f", a/b}')" "$mf" \
        "$(awk -v r="$mr" 'BEGIN{printf "%.1f", r/1024}')"
    fi
  done
done

rm -rf "$SCRATCH"
echo
echo "appended to $CSV"
