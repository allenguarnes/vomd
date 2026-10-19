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
#   settings E154, ellipsis E0B6, image E0F6 (remote images, P10), unfold-horizontal E43D and
#   fold-horizontal E43B (wide layout on and off), folder-open E247 (open a file). Buttons of
#   later phases add theirs when they land: list E106 (ToC, v1.2), search E151 (find).
icons="U+E048,U+E11C,U+E13D,U+E09D,U+E149,U+E154,U+E0B6,U+E0F6,U+E43D,U+E43B,U+E247"

# The tables a renderer doesn't need are dropped (OS/2, name, post), and the em is cut from 1000
# to 128 units: 8 units per pixel at toolbar sizes, and the smaller outline deltas take fewer bytes.
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
pyftsubset "$src/font/lucide.ttf" --output-file="$tmp" --unicodes="$icons" \
  --layout-features='' --no-hinting --drop-tables+=DSIG,STAT,GSUB,GPOS,FFTM,OS/2,name,post \
  --notdef-outline
fonttools ttLib.scaleUpem --output-file="$out/icons.ttf" "$tmp" 128
cp "$src/LICENSE" "$out/LICENSE-Lucide.txt"
printf '%-28s %7d bytes\n' icons.ttf "$(stat -c %s "$out/icons.ttf")"
