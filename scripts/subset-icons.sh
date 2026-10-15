#!/usr/bin/env bash
# Regenerate the toolbar icon font assets/fonts/icons.ttf.
#
# Usage: scripts/subset-icons.sh <dir-with-extracted-lucide-static-package>
#   lucide-static 1.57.0  https://registry.npmjs.org/lucide-static/-/lucide-static-1.57.0.tgz
#
# Requires fonttools (pyftsubset). The output is committed; this never runs at build time.
# Lucide's own private-use code points are kept, so `src/toolbar.rs` names them directly.
set -euo pipefail
src=${1:?lucide-static package dir}
out=$(cd "$(dirname "$0")/.." && pwd)/assets/fonts

#   arrow-left E048, minus E11C, plus E13D, contrast E09D (theme), rotate-cw E149 (reload),
#   settings E154, ellipsis E0B6. Buttons of later phases add theirs when they land: list E106
#   (ToC, v1.2), image E0F6 (remote images, P10), search E151 (find).
icons="U+E048,U+E11C,U+E13D,U+E09D,U+E149,U+E154,U+E0B6"

pyftsubset "$src/font/lucide.ttf" --output-file="$out/icons.ttf" --unicodes="$icons" \
  --layout-features='' --no-hinting --drop-tables+=DSIG,STAT,GSUB,GPOS,FFTM \
  --name-IDs=0,1,2,3,4,5,6 --notdef-outline
cp "$src/LICENSE" "$out/LICENSE-Lucide.txt"
printf '%-28s %7d bytes\n' icons.ttf "$(stat -c %s "$out/icons.ttf")"
