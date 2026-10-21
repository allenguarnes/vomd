#!/usr/bin/env bash
# Run a fuzz target (fuzz/fuzz_targets/) on the stable toolchain: libFuzzer with the same
# coverage instrumentation cargo-fuzz uses, without a sanitizer (cargo-fuzz needs nightly for
# ASan; CI's nightly job uses it). Usage: scripts/fuzz.sh <doc|image|layout|settings|svg> [seconds]
set -euo pipefail
target=${1:?usage: scripts/fuzz.sh <doc|image|layout|settings|svg> [seconds]}
secs=${2:-60}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root/fuzz"
triple=x86_64-unknown-linux-gnu
RUSTFLAGS="--cfg fuzzing -Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=4 \
-Cllvm-args=-sanitizer-coverage-inline-8bit-counters -Cllvm-args=-sanitizer-coverage-pc-table \
-Cllvm-args=-sanitizer-coverage-trace-compares -Cdebug-assertions -Coverflow-checks" \
    cargo build --release --locked --target "$triple" --bin "$target"
# Longer inputs only slow the runs down: a layout pass is far dearer than a parse.
case $target in
    doc | image | svg) max_len=65536 ;;
    layout) max_len=16384 ;;
    *) max_len=4096 ;;
esac
# An SVG that takes long to draw is what the `svg` target looks for.
timeout=10
extra=()
if [ "$target" = svg ]; then
    timeout=2
    extra=(-dict=svg.dict)
fi
corpus="corpus/$target"
mkdir -p "$corpus" "artifacts/$target"
# Seeds: the fixtures for the Markdown targets, the images for `image` (the SVGs for `svg`), and
# seeds/<target>/: the sample files of the
# settings, and the inputs of past findings (also run as tests). The seeds go in every run, so
# a finding is run also from a corpus that was seeded before it.
if [ "$target" = image ] && [ -z "$(ls -A "$corpus")" ]; then
    cp "$root"/fixtures/images/* "$corpus/"
elif [ "$target" = svg ] && [ -z "$(ls -A "$corpus")" ]; then
    cp "$root"/fixtures/images/*.svg* "$corpus/"
elif [ "$target" != settings ] && [ -z "$(ls -A "$corpus")" ]; then
    for f in "$root"/fixtures/*.md; do
        if [ "$(stat -c %s "$f")" -le "$max_len" ]; then cp "$f" "$corpus/"; fi
    done
fi
if [ -d "seeds/$target" ]; then cp seeds/"$target"/* "$corpus/"; fi
exec "target/$triple/release/$target" "$corpus" \
    -artifact_prefix="artifacts/$target/" -max_total_time="$secs" -max_len="$max_len" \
    -timeout="$timeout" -rss_limit_mb=2048 -print_final_stats=1 "${extra[@]}"
