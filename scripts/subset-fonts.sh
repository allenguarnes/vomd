#!/usr/bin/env bash
# Regenerate the embedded font subsets in assets/fonts/.
#
# Usage: scripts/subset-fonts.sh <dir-with-extracted-release-zips>
#   Inter 4.1          https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip
#   JetBrains Mono 2.304 https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip
#
# Requires fonttools (pyftsubset). Outputs are committed; this never runs at build time.
set -euo pipefail
src=${1:?source dir}
out=$(cd "$(dirname "$0")/.." && pwd)/assets/fonts

# Coverage is deliberately narrow to stay within the ~450 KB budget. Anything else
# (Greek, Cyrillic, Latin Ext-B, CJK, emoji, ...) comes from the lazy system-font fallback.
#   sans: Latin + Ext-A, punctuation, currency, letterlike, arrows, math, geometric shapes, check marks
#   mono: Latin + Ext-A, punctuation, arrows, box drawing + block elements (common in code/diagrams)
sans="U+0020-017F,U+2000-206F,U+20A0-20CF,U+2100-214F,U+2190-21FF,U+2200-22FF,U+25A0-25FF,\
U+2713-2717,U+FB01-FB02,U+FFFD"
mono="U+0020-017F,U+2000-206F,U+2190-21FF,U+2500-259F,U+FFFD"

# Layout features: Inter keeps its contextual forms (calt). JetBrains Mono leaves calt out, which
# is where its code ligatures (->, !=, >= ...) live: code shows the characters typed, as GitHub
# does, and the two mono files are about 34 KB smaller each.
sans_features='kern,liga,calt,ccmp,locl,mark,mkmk,tnum'
mono_features='kern,liga,ccmp,locl,mark,mkmk,tnum'

subset() { # <ranges> <features> <input.ttf> <output.ttf>
  pyftsubset "$3" --output-file="$4" --unicodes="$1" \
    --layout-features="$2" \
    --no-hinting --drop-tables+=DSIG,STAT --name-IDs=0,1,2,3,4,5,6 --notdef-outline
  printf '%-28s %7d bytes\n' "$(basename "$4")" "$(stat -c %s "$4")"
}

subset "$sans" "$sans_features" "$src/inter/extras/ttf/Inter-Regular.ttf"    "$out/Inter-Regular.ttf"
subset "$sans" "$sans_features" "$src/inter/extras/ttf/Inter-Bold.ttf"       "$out/Inter-Bold.ttf"
subset "$sans" "$sans_features" "$src/inter/extras/ttf/Inter-Italic.ttf"     "$out/Inter-Italic.ttf"
subset "$sans" "$sans_features" "$src/inter/extras/ttf/Inter-BoldItalic.ttf" "$out/Inter-BoldItalic.ttf"
subset "$mono" "$mono_features" "$src/jbm/fonts/ttf/JetBrainsMono-Regular.ttf" "$out/JetBrainsMono-Regular.ttf"
subset "$mono" "$mono_features" "$src/jbm/fonts/ttf/JetBrainsMono-Bold.ttf"    "$out/JetBrainsMono-Bold.ttf"
cp "$src/inter/LICENSE.txt" "$out/LICENSE-Inter.txt"
cp "$src/jbm/OFL.txt"       "$out/LICENSE-JetBrainsMono.txt"
printf '%-28s %7d bytes\n' TOTAL "$(cat "$out"/*.ttf | wc -c)"
