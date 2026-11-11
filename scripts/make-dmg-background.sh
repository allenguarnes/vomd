#!/usr/bin/env bash
# Make the background of the macOS disk image's window, packaging/macos/dmg-background.png and
# its @2x, from packaging/macos/dmg-background.svg.
#
# Usage: scripts/make-dmg-background.sh
#
# Requires rsvg-convert and (optionally) oxipng. The output is committed; `cargo xtask dist`
# joins the two into one HiDPI TIFF (`tiffutil`) when it makes the disk image.
set -euo pipefail
dir=$(cd "$(dirname "$0")/.." && pwd)/packaging/macos
rsvg-convert -w 600 -h 400 "$dir/dmg-background.svg" -o "$dir/dmg-background.png"
rsvg-convert -w 1200 -h 800 "$dir/dmg-background.svg" -o "$dir/dmg-background@2x.png"
if command -v oxipng >/dev/null; then
  oxipng -q -o max --strip all "$dir/dmg-background.png" "$dir/dmg-background@2x.png"
fi
ls -l "$dir"/dmg-background*.png
