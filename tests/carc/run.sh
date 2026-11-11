#!/bin/sh
# Continual-ARC on floppy (tests/carc/README.md). Run from the floppy checkout.
#
#   sh tests/carc/run.sh probe    # one instance: does a grid make it through a reply?
#   sh tests/carc/run.sh stream   # the arc1-smoke stream, floppy keeping what it learns
#   sh tests/carc/run.sh reset    # the same stream, floppy wiped before every instance
#   sh tests/carc/run.sh learned  # the share learned: stream against reset
#
# FLOPPY_CARC_ENV (default .env) is the base env file: its FLOPPY_REFLECTION_BASE_URL
# and key (the hosted gateway) and TYPESAFE_* are all floppy is given.
# FLOPPY_CARC_MODEL (default gpt-6-luna) is every model floppy calls.
# FLOPPY_CARC_CONFIG (default arc1-smoke) and FLOPPY_CARC_SEED (default 0).
set -eu
ROOT=$(pwd)
OUT="$ROOT/target/floppy-carc"
CARC="$OUT/continual-arc"
PIN=d1fd7170de442fc5aa91a5b7426914f274052b26
CONFIG=${FLOPPY_CARC_CONFIG:-arc1-smoke}
SEED=${FLOPPY_CARC_SEED:-0}
export FLOPPY_CARC_ENV=${FLOPPY_CARC_ENV:-$ROOT/.env}
export FLOPPY_CARC_MODEL=${FLOPPY_CARC_MODEL:-gpt-6-luna}
mkdir -p "$OUT"

if [ ! -d "$CARC/.git" ]; then
  git clone --filter=blob:none https://github.com/unifyai/continual-arc "$CARC"
fi
git -C "$CARC" checkout -q "$PIN"
if [ ! -x "$CARC/.venv/bin/continual-arc" ]; then
  (cd "$CARC" && uv venv -q && uv pip install -q -e . -e baselines)
fi
BIN="$CARC/.venv/bin"
"$BIN/continual-arc" data fetch >/dev/null

STREAM="$OUT/$CONFIG-$SEED-stream.json"
RESET="$OUT/$CONFIG-$SEED-reset.json"
[ -f "$STREAM" ] || "$BIN/continual-arc" schedule make --config "$CONFIG" --seed "$SEED" --no-epilogue --out "$STREAM" >/dev/null
[ -f "$RESET" ] || "$BIN/continual-arc" schedule make --config "$CONFIG" --seed "$SEED" --no-epilogue --reset-each --out "$RESET" >/dev/null

run() {  # run NAME SCHEDULE [extra args]
  name=$1; schedule=$2; shift 2
  FLOPPY_CARC_WORK="$OUT/$name-work" PYTHONPATH="$ROOT" \
    "$BIN/continual-arc" run --learner tests.carc.floppy_learner:FloppyLearner \
    --schedule "$schedule" --out "$OUT/$name" "$@"
  "$BIN/continual-arc" report "$OUT/$name"
}

case "${1:-}" in
  probe)
    PROBE="$OUT/$CONFIG-$SEED-probe.json"
    [ -f "$PROBE" ] || "$BIN/continual-arc" schedule make --config "$CONFIG" --seed "$SEED" --no-epilogue --rounds 1 --out "$PROBE" >/dev/null
    rm -rf "$OUT/probe" "$OUT/probe-work"
    run probe "$PROBE" --events
    ;;
  stream) rm -rf "$OUT/stream" "$OUT/stream-work"; run stream "$STREAM" --events ;;
  reset) rm -rf "$OUT/reset" "$OUT/reset-work"; run reset "$RESET" --events ;;
  learned) "$BIN/continual-arc" learned "$OUT/stream" "$OUT/reset" ;;
  *) sed -n 2,13p "$0"; exit 2 ;;
esac
