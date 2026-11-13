#!/usr/bin/env bash
# Make the macOS app icon packaging/macos/vomd.icns from assets/icon.svg.
#
# Usage: scripts/make-icns.sh
#
# Requires rsvg-convert, python3 and (optionally) oxipng; runs on Linux or macOS. The output is
# committed; this never runs at build time. The icon is a rounded square with room around it
# already, which is what a macOS icon is. An .icns file is its PNGs, each with a four-letter type
# and its length (iconutil makes the same from an .iconset folder).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# (type, pixels): the sizes of an .iconset, 128, 256 and 512 and each one's @2x, and the @2x of
# 16 and 32. Not the 16 and 32 themselves (icp4, icp5): macOS's icon service draws those PNGs as
# noise (iconutil writes them in an older format instead); without them it scales the others.
types="ic07:128 ic08:256 ic09:512 ic10:1024 ic11:32 ic12:64 ic13:256 ic14:512"
for t in $types; do
  px=${t#*:}
  [ -f "$tmp/$px.png" ] && continue
  rsvg-convert -w "$px" -h "$px" "$root/assets/icon.svg" -o "$tmp/$px.png"
  if command -v oxipng >/dev/null; then oxipng -q -o max --strip all "$tmp/$px.png"; fi
done

python3 - "$tmp" "$root/packaging/macos/vomd.icns" $types <<'PY'
import struct, sys
tmp, out, types = sys.argv[1], sys.argv[2], sys.argv[3:]
chunks = b""
for t in types:
    kind, px = t.split(":")
    png = open(f"{tmp}/{px}.png", "rb").read()
    chunks += kind.encode() + struct.pack(">I", 8 + len(png)) + png
open(out, "wb").write(b"icns" + struct.pack(">I", 8 + len(chunks)) + chunks)
PY
ls -l "$root/packaging/macos/vomd.icns"
