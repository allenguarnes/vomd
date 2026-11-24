#!/bin/sh
# The two color fonts of the tests (fixtures/fonts/), never embedded:
# - test-emoji-colr.ttf: VOMD's own COLRv0 font (fontTools): 🚀 red, 👍 green, 🏽 blue, and a ccmp
#   ligature drawing 👍🏽 as one glyph of both colors. No third-party licence.
# - test-emoji-cbdt.ttf: Noto Color Emoji (OFL 1.1, LICENSE-NotoColorEmoji.txt), subset to 🚀 👍 🏽
#   ❤ and the variation selector and joiner: CBDT bitmaps, drawn through VOMD's raster hook.
# - test-marks.ttf: VOMD's own plain font (fontTools) with A and U+1AB0 (a combining mark the
#   embedded faces lack), for a mark found in a fallback with its base. No third-party licence.
# - kin/Inter-Regular.ttf, kin/JetBrainsMono-Regular.ttf: VOMD's own plain fonts named for the
#   embedded families, with Ж (which their subsets lack), for a scan that finds both installed.
# Needs Python 3 with fontTools (pyftsubset); NOTO is the installed NotoColorEmoji.ttf.
set -eu
out="$(dirname "$0")/../fixtures/fonts"
NOTO="${NOTO:-/usr/share/fonts/noto/NotoColorEmoji.ttf}"
python3 - "$out/test-emoji-colr.ttf" <<'PY'
import sys
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.feaLib.builder import addOpenTypeFeaturesFromString

def box(x0, y0, x1, y1):
    p = TTGlyphPen(None)
    p.moveTo((x0, y0)); p.lineTo((x0, y1)); p.lineTo((x1, y1)); p.lineTo((x1, y0)); p.closePath()
    return p.glyph()

names = [".notdef", "rocket", "thumb", "tone", "thumb_tone"]
fb = FontBuilder(1000, isTTF=True)
fb.setupGlyphOrder(names)
fb.setupCharacterMap({0x1F680: "rocket", 0x1F44D: "thumb", 0x1F3FD: "tone"})
fb.setupGlyf({n: box(50, -100, 1150, 900) for n in names})
fb.setupHorizontalMetrics({n: (1200, 50) for n in names})
fb.setupHorizontalHeader(ascent=900, descent=-100)
# (fontdb refuses a font without a PostScript name, name ID 6.)
fb.setupNameTable({"familyName": "VOMD Test Emoji", "styleName": "Regular",
                   "psName": "VOMDTestEmoji-Regular"})
fb.setupOS2(sTypoAscender=900, sTypoDescender=-100, usWinAscent=900, usWinDescent=100,
            sxHeight=500, sCapHeight=700, fsSelection=0x40)
fb.setupPost()
fb.setupCOLR({"rocket": [("rocket", 0)], "thumb": [("thumb", 1)], "tone": [("tone", 2)],
              "thumb_tone": [("thumb", 1), ("tone", 2)]}, version=0)
fb.setupCPAL([[(1, 0, 0, 1), (0, 0.6, 0, 1), (0, 0, 1, 1)]])
addOpenTypeFeaturesFromString(fb.font, "feature ccmp { sub thumb tone by thumb_tone; } ccmp;")
fb.save(sys.argv[1])
PY
mkdir -p "$out/kin"
python3 - "$out/test-marks.ttf" "$out/kin" <<'PY'
import sys
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

def box(x0, y0, x1, y1):
    p = TTGlyphPen(None)
    p.moveTo((x0, y0)); p.lineTo((x0, y1)); p.lineTo((x1, y1)); p.lineTo((x1, y0)); p.closePath()
    return p.glyph()

fb = FontBuilder(1000, isTTF=True)
fb.setupGlyphOrder([".notdef", "A", "mark"])
fb.setupCharacterMap({0x41: "A", 0x1AB0: "mark"})
fb.setupGlyf({".notdef": box(50, 0, 550, 700), "A": box(50, 0, 550, 700),
              "mark": box(-450, 750, -50, 900)})
fb.setupHorizontalMetrics({".notdef": (600, 50), "A": (600, 50), "mark": (0, -450)})
fb.setupHorizontalHeader(ascent=900, descent=-100)
fb.setupNameTable({"familyName": "VOMD Test Marks", "styleName": "Regular",
                   "psName": "VOMDTestMarks-Regular"})
fb.setupOS2(sTypoAscender=900, sTypoDescender=-100, usWinAscent=900, usWinDescent=100,
            sxHeight=500, sCapHeight=700, fsSelection=0x40)
fb.setupPost()
fb.save(sys.argv[1])

for family, ps, width in (("Inter", "Inter-Regular", 600), ("JetBrains Mono", "JetBrainsMono-Regular", 500)):
    fb = FontBuilder(1000, isTTF=True)
    fb.setupGlyphOrder([".notdef", "zhe"])
    fb.setupCharacterMap({0x416: "zhe"})
    fb.setupGlyf({n: box(50, 0, width - 50, 700) for n in (".notdef", "zhe")})
    fb.setupHorizontalMetrics({n: (width, 50) for n in (".notdef", "zhe")})
    fb.setupHorizontalHeader(ascent=900, descent=-100)
    fb.setupNameTable({"familyName": family, "styleName": "Regular", "psName": ps})
    fb.setupOS2(sTypoAscender=900, sTypoDescender=-100, usWinAscent=900, usWinDescent=100,
                sxHeight=500, sCapHeight=700, fsSelection=0x40)
    fb.setupPost()
    fb.save(f"{sys.argv[2]}/{ps}.ttf")
PY
pyftsubset "$NOTO" --unicodes=U+1F680,U+1F44D,U+1F3FD,U+2764,U+FE0F,U+200D \
    --output-file="$out/test-emoji-cbdt.ttf"
