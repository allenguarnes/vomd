#!/usr/bin/env bash
# Regenerate the embedded math font assets/fonts/STIXTwoMath-Regular.otf.
#
# Usage: scripts/subset-math-font.sh <dir-with-STIXTwoMath-Regular.otf>
#   STIX Two Math 2.13  https://github.com/stipub/stixfonts (fonts/static_otf/STIXTwoMath-Regular.otf)
#
# Requires fonttools (pyftsubset). The output is committed; this never runs at build time.
# The code points are those of assets/fonts/math-codepoints.txt. The MATH table is kept, and
# pyftsubset keeps every glyph it reaches: size variants and assembly parts of the delimiters,
# radicals, big operators and accents. No GSUB feature is kept: `ssty` (script-size alternates)
# would double the outlines, and neither KaTeX nor MathJax uses it. The font's license is
# assets/fonts/LICENSE-STIXTwoMath.txt.
set -euo pipefail
src=${1:?dir with STIXTwoMath-Regular.otf}
out=$(cd "$(dirname "$0")/.." && pwd)/assets/fonts

pyftsubset "$src/STIXTwoMath-Regular.otf" --output-file="$out/STIXTwoMath-Regular.otf" \
  --unicodes-file="$out/math-codepoints.txt" --layout-features='' \
  --no-hinting --drop-tables+=DSIG,STAT,FFTM --name-IDs=0,1,2,3,4,5,6,13,14 --notdef-outline
printf '%-28s %7d bytes\n' STIXTwoMath-Regular.otf "$(stat -c %s "$out/STIXTwoMath-Regular.otf")"
