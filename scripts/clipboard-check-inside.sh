#!/usr/bin/env bash
# Run by scripts/clipboard-check.sh inside its container, never on a desktop: it starts its own
# Xvfb and sway, and drives VOMD with real key and mouse events (xdotool on X11, wtype and
# sway's own `seat cursor` commands on Wayland) and pastes with the real programs (xclip, xsel,
# wl-paste), so the whole path is the one a user's is: key → selection → copy → server →
# program. On each of X11 and Wayland it checks that:
#   - Ctrl+A offers the document as PRIMARY, and Ctrl+C as CLIPBOARD, as plain text with the
#     list marker, the quote mark and the table's tab, and with Ctrl+C and nothing selected the
#     clipboard is left as it was;
#   - the targets (X11) or types (Wayland) include UTF-8 text, and `STRING` is Latin-1;
#   - (X11 only) double-click and triple-click select a word and a paragraph and offer them as
#     PRIMARY, and a drag offers something it selected: a headless sway has no pointer device for
#     a client to hear (the one wlrctl makes lives as long as the command), so on Wayland the
#     selections are made by keyboard;
#   - (X11 only, for the same reason) pointing at a code block and clicking the copy button
#     that shows in its corner makes the clipboard exactly the code (tab, blank line,
#     indentation, no final line break), and leaves PRIMARY and what Ctrl+A selected alone;
#     a click beside the button copies nothing; the Wayland path of such a copy is covered by a
#     unit test (`a_pasted_code_block_is_its_code_to_the_byte`);
#   - the answers come after 3 s of idle, and the idle window used no CPU for them;
#   - another program taking the selection ends VOMD's offer, and a new Ctrl+C gets it back;
#   - a 5 MB document copies whole (X11's INCR, Wayland's pipe), byte for byte, to two programs
#     at once, without VOMD's memory growing by the text;
#   - with `primary_selection = off`, PRIMARY is never set, and the clipboard still works;
#   - (X11 only) the rarer requests: MULTIPLE, TIMESTAMP, and one from before VOMD owned the
#     selection (refused), made by `examples/clip_probe.rs`; and at exit a clipboard manager
#     (also the probe) is asked to save the clipboard, and does, while with none VOMD quits at
#     once;
#   - reading, which a paste into the find bar's field does: what another program (xclip,
#     wl-copy) offers as CLIPBOARD and PRIMARY is read by VOMD's own code, in a window of the
#     probe's (`clip_probe read`), whole, with the first 64 KiB of a long one (X11's INCR too),
#     in Latin-1 when that is all that is offered (X11), with nothing printed for a selection
#     that has no owner, offers no text, or is never answered (X11: after about 2 s), and with
#     what the probe offers itself read back as it is; and in VOMD itself, with real keys:
#     Ctrl+F, Ctrl+V (the clipboard, X11 and Wayland) or a middle click on the field (PRIMARY,
#     X11 only, for the reason above), Ctrl+A and Ctrl+C to copy what was pasted out, a
#     selection VOMD offers itself read back the same way, and the field's own Ctrl+X, Ctrl+A
#     (PRIMARY) and Ctrl+V.
set -euo pipefail
[ "${VOMD_CLIPBOARD_CHECK:-}" = in-container ] || {
    echo "run scripts/clipboard-check.sh: this starts servers and sends input, and must stay in its container"
    exit 2
}
vomd=${VOMD_BIN:?VOMD_BIN}
probe=${VOMD_PROBE:-$(dirname "$vomd")/clip_probe}
work=$(mktemp -d /tmp/clipcheck.XXXXXX)
cd "$work"
suites=${*:-x11 wayland}

fail() {
    echo "FAIL: $*"
    exit 1
}
ok() { echo "ok: $*"; }

# The documents, and the text that selecting all of each must give.
printf '%s\n' 'Paragraph one: café 日本 and some more words.' '' '- item *one*' '- item two' '' \
    '> quoted line' '' '| a | b |' '|---|---|' '| 1 | 2 |' >small.md
printf 'Paragraph one: café 日本 and some more words.\n\n- item one\n- item two\n\n> quoted line\n\na\tb\n1\t2' >small.expected
printf 'Paragraph one: caf\351 ?? and some more words.\n\n- item one\n- item two\n\n> quoted line\n\na\tb\n1\t2' >small.latin1
printf 'Paragraph one: café 日本 and some more words.' >first.expected
printf 'Paragraph' >word.expected
# What other programs offer for VOMD to read: UTF-8 text with a second line, Latin-1 text that is
# `STRING` alone, and the text VOMD reads of a long one (64 KiB).
printf 'café 日本 first line\nsecond line' >offer.txt
printf 'café 日本 first line' >offer.first
printf 'caf\351' >offer.latin1
printf 'café' >offer.latin1.expected
printf 'hello world' >hello.expected
printf 'OTHER' >other.expected
# A document that starts with code, for the copy button: the code has a tab, a blank line, an
# indented line and a non-ASCII letter, and the text that selecting all of it gives.
printf '%s\n' '```rust' 'fn f() {' $'\treturn 1;  // tab' '' '    deep   spaced é' '}' '```' '' 'After the code.' >code.md
printf 'fn f() {\n\treturn 1;  // tab\n\n    deep   spaced é\n}' >code.expected
printf 'fn f() {\n\treturn 1;  // tab\n\n    deep   spaced \351\n}' >code.latin1
{ cat code.expected; printf '\n\nAfter the code.'; } >code.all
awk -v n=90000 'BEGIN { for (i = 1; i <= n; i++) { if (i > 1) printf "\n\n"; printf "Paragraph %d of the big document, with a little text in it.", i } }' >big.expected
{ cat big.expected; echo; } >big.md
big_sha=$(sha256sum <big.expected | cut -d' ' -f1)
head -c 65536 big.expected >big.cap
echo "== a $(stat -c %s big.md) byte document, sha256 $big_sha"
mkdir -p cfg-on cfg-off home run
printf 'primary_selection = off\n' >cfg-off/settings.ini
mkdir -p cfg-on/vomd cfg-off/vomd
mv cfg-off/settings.ini cfg-off/vomd/settings.ini

# Whether the process `$1` is over (a child that ended stays in /proc until it is waited for).
exited() { [ ! -d "/proc/$1" ] || [ "$(awk '{ print $3 }' "/proc/$1/stat")" = Z ]; }

ticks() { awk '{ print $14 + $15 }' "/proc/$1/stat"; }
rss_kb() { awk '/^VmRSS:/ { print $2 }' "/proc/$1/status"; }
peak_kb() { awk '/^VmHWM:/ { print $2 }' "/proc/$1/status"; }

# Run `cmd` until it succeeds or 3 s are up (a copy reaches the server a moment after the key).
retry() {
    local _
    for _ in $(seq 30); do
        "$@" && return 0
        sleep 0.1
    done
    return 1
}

pids=()
cleanup() {
    local p
    for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
}
trap cleanup EXIT

# --- X11 ---------------------------------------------------------------------------------------

x11_server() {
    Xvfb :99 -screen 0 1280x1024x24 -nolisten tcp -ac >xvfb.log 2>&1 &
    pids+=($!)
    retry test -S /tmp/.X11-unix/X99 || fail "Xvfb didn't start"
}

# VOMD on X11 with the given config dir, showing `file`; sets $pid, $win and the window's
# geometry. Only the X server is reachable: no Wayland display.
x11_vomd() { # file config
    env -u WAYLAND_DISPLAY DISPLAY=:99 XDG_CONFIG_HOME="$work/$2" XDG_RUNTIME_DIR="$work/run" \
        HOME="$work/home" "$vomd" "$1" >vomd.log 2>&1 &
    pid=$!
    pids+=("$pid")
    win=""
    local _
    for _ in $(seq 100); do
        win=$(DISPLAY=:99 xdotool search --onlyvisible --name "^$1\$" 2>/dev/null | head -1) || true
        [ -n "$win" ] && break
        sleep 0.1
    done
    [ -n "$win" ] || fail "no VOMD window for $1: $(cat vomd.log)"
    DISPLAY=:99 xdotool windowfocus --sync "$win"
    eval "$(DISPLAY=:99 xdotool getwindowgeometry --shell "$win")"
    sleep 1
}

x11_end() {
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    retry test -z "$(DISPLAY=:99 xdotool search --name '.' 2>/dev/null)" || true
}

x11_key() { DISPLAY=:99 xdotool key "$@"; }

# Where the first line's text starts, and its height's middle, in screen coordinates: the
# toolbar and the page's padding above it, the column centered (720 px, or the window less its
# margins).
text_x() {
    local col=720
    [ "$WIDTH" -lt 784 ] && col=$((WIDTH - 64))
    echo $((X + (WIDTH - col) / 2))
}
text_y() { echo $((Y + 32 + 32 + 12)); }

x11_pointer() { DISPLAY=:99 xdotool mousemove "$(($(text_x) + $1))" "$(text_y)"; }

# The page's first block is a code block at 100% zoom and scale 1: its top is under the toolbar
# (32 px) and the page's padding (32 px), its right edge is the column's, and its copy button is
# 28 px square, 8 px in from the corner. The block's own text is at the left, and its middle
# (the first line's, as `text_y` finds it) is over the block but not the button.
button_x() {
    local col=720
    [ "$WIDTH" -lt 784 ] && col=$((WIDTH - 64))
    echo $((X + (WIDTH + col) / 2 - 8 - 14))
}
button_y() { echo $((Y + 32 + 32 + 8 + 14)); }

# `paste sel`: what a program gets from the selection (clipboard or primary); `same sel file`.
x11_paste() { xclip -o -selection "$1" 2>/dev/null; }
x11_same() { # sel, file
    x11_paste "$1" >got.txt && cmp -s got.txt "$2"
}
x11_unset() { ! xclip -o -selection "$1" >got.txt 2>/dev/null && [ ! -s got.txt ]; }

# What the probe's window reads from the selection `$1` with VOMD's code (`$2`: it offers that
# first, itself): its output is the text, and it fails with nothing printed if there was none.
x11_read() { DISPLAY=:99 timeout 30 "$probe" read "$@"; }

# Where the find bar's field is in the window: the bar is below the toolbar (32 px), as high as it.
field_x() { echo $((X + 60)); }
field_y() { echo $((Y + 32 + 16)); }

x11_reading() {
    echo "-- reading"
    local t0 t1
    # No owner (nothing has been offered on this server yet): nothing, and without a wait.
    t0=$(date +%s%N)
    x11_read PRIMARY >got.txt 2>err.txt && fail "X11: read a PRIMARY that has no owner: $(cat got.txt)"
    t1=$(date +%s%N)
    [ ! -s got.txt ] || fail "X11: a selection with no owner gave '$(cat got.txt)'"
    grep -qx nothing err.txt || fail "X11: no owner: $(cat err.txt)"
    [ $(((t1 - t0) / 1000000)) -lt 1500 ] || fail "X11: a selection with no owner took $(((t1 - t0) / 1000000)) ms"
    ok "X11: a selection with no owner reads as nothing, at once"
    # Another program's UTF-8 text, CLIPBOARD and PRIMARY, whole (the first line is the field's doing).
    xclip -i -selection clipboard <offer.txt
    x11_read CLIPBOARD >got.txt || fail "X11: reading CLIPBOARD: $(cat err.txt)"
    cmp -s got.txt offer.txt || fail "X11: CLIPBOARD read as: $(od -c got.txt | head -5)"
    xclip -i -selection primary <offer.txt
    x11_read PRIMARY >got.txt || fail "X11: reading PRIMARY"
    cmp -s got.txt offer.txt || fail "X11: PRIMARY read as: $(od -c got.txt | head -5)"
    ok "X11: VOMD's code reads CLIPBOARD and PRIMARY that xclip offers"
    # A program that offers only STRING (Latin-1) is read through the fallback.
    xclip -i -selection clipboard -t STRING <offer.latin1
    x11_read CLIPBOARD >got.txt || fail "X11: reading a STRING-only clipboard"
    cmp -s got.txt offer.latin1.expected || fail "X11: STRING read as: $(od -c got.txt | head -3)"
    ok "X11: a clipboard that is Latin-1 STRING alone is read"
    # A long one comes by INCR (xclip's 5 MB does): the first 64 KiB, and nothing waits for the rest.
    xclip -i -selection clipboard <big.expected
    t0=$(date +%s%N)
    x11_read CLIPBOARD >got.txt || fail "X11: reading a 5 MB clipboard"
    t1=$(date +%s%N)
    cmp -s got.txt big.cap || fail "X11: the 5 MB clipboard read as $(stat -c %s got.txt) bytes"
    [ $(((t1 - t0) / 1000000)) -lt 3000 ] || fail "X11: reading 5 MB took $(((t1 - t0) / 1000000)) ms"
    ok "X11: a 5 MB clipboard is read up to 64 KiB"
    # Two reads in a row from it: the first leaves the owner's transfer unfinished, and the second
    # has none of its pieces (nor any of the first's) in its text.
    # (A new xclip: the one that served the read before is still waiting for a window that is gone.)
    xclip -i -selection clipboard <big.expected
    DISPLAY=:99 timeout 30 "$probe" reread CLIPBOARD >got2.bin || fail "X11: reading a 5 MB clipboard twice"
    head -c 65536 got2.bin | cmp -s - big.cap || fail "X11: the first of two 5 MB reads differs"
    tail -c +65538 got2.bin | cmp -s - big.cap || fail "X11: the second of two 5 MB reads differs"
    [ "$(stat -c %s got2.bin)" = 131073 ] || fail "X11: two 5 MB reads came to $(stat -c %s got2.bin) bytes"
    ok "X11: a second read after a 5 MB one has nothing of its transfer in it"
    # An owner that never answers is given up on after a moment, without a text.
    "$probe" hold CLIPBOARD >hold.log &
    local holder=$!
    pids+=("$holder")
    retry grep -qx ready hold.log || fail "X11: the probe didn't take the clipboard"
    t0=$(date +%s%N)
    x11_read CLIPBOARD >got.txt 2>err.txt && fail "X11: read from an owner that never answers: $(cat got.txt)"
    t1=$(date +%s%N)
    kill "$holder" 2>/dev/null || true
    wait "$holder" 2>/dev/null || true
    grep -qx nothing err.txt || fail "X11: a stuck owner: $(cat err.txt)"
    [ $(((t1 - t0) / 1000000)) -ge 1500 ] || fail "X11: gave up on a stuck owner after $(((t1 - t0) / 1000000)) ms"
    [ $(((t1 - t0) / 1000000)) -lt 6000 ] || fail "X11: waited $(((t1 - t0) / 1000000)) ms for a stuck owner"
    ok "X11: an owner that never answers is given up on in about 2 s"
    # An owner that is slow with the pieces of an INCR transfer (it writes the next a moment after
    # the reader deleted the last one) is read whole: nothing may take a piece away in between.
    "$probe" slow CLIPBOARD 'slow owner' 30 >slow.log &
    holder=$!
    pids+=("$holder")
    retry grep -qx ready slow.log || fail "X11: the slow probe didn't take the clipboard"
    x11_read CLIPBOARD >got.txt 2>err.txt || fail "X11: reading from a slow INCR owner: $(cat err.txt)"
    [ "$(cat got.txt)" = 'slow owner' ] || fail "X11: a slow INCR owner read as '$(cat got.txt)'"
    retry grep -qx ended slow.log || fail "X11: a slow INCR owner's transfer wasn't ended by the reader"
    kill "$holder" 2>/dev/null || true
    wait "$holder" 2>/dev/null || true
    ok "X11: an INCR owner that is slow with each piece is read whole"
    # An owner that answers late, after the paste gave up on it: the answer goes to a property of
    # the old paste, which VOMD keeps emptying so that the owner can end its transfer, and the
    # next paste (right after the first gave up) has its own text, not a piece of the late one.
    "$probe" stale CLIPBOARD 'fresh text' >stale.log &
    holder=$!
    pids+=("$holder")
    retry grep -qx ready stale.log || fail "X11: the stale probe didn't take the clipboard"
    DISPLAY=:99 timeout 30 "$probe" again CLIPBOARD >got2.bin 2>err.txt || fail "X11: reading from a late owner: $(cat err.txt)"
    printf '\0fresh text' | cmp -s - got2.bin || fail "X11: two reads, the first given up on, came to: $(od -c got2.bin | head -3)"
    retry grep -qx ended stale.log || fail "X11: a late answer was not taken from the property of the paste that gave up"
    kill "$holder" 2>/dev/null || true
    wait "$holder" 2>/dev/null || true
    ok "X11: a late answer to a paste that gave up neither reaches the next paste nor stalls its owner"
    # A selection the reader offers itself is read back as it is.
    x11_read CLIPBOARD 'own text é' >got.txt || fail "X11: reading its own selection"
    [ "$(cat got.txt)" = 'own text é' ] || fail "X11: its own selection read as '$(cat got.txt)'"
    ok "X11: a selection VOMD's code offers itself reads back from what it offers"

    echo "-- pasting into the find bar"
    # Ctrl+F, Ctrl+V, then Ctrl+A and Ctrl+C copy the query out: what was pasted, its first line.
    xclip -i -selection clipboard <offer.txt
    x11_vomd small.md cfg-on
    x11_key ctrl+f
    sleep 0.3
    x11_key ctrl+v
    sleep 0.5
    x11_key ctrl+a ctrl+c
    retry x11_same clipboard offer.first || fail "X11: Ctrl+V into the find bar: the query came out as '$(x11_paste clipboard | od -c | head -3)'"
    ok "X11: Ctrl+V pastes the clipboard's first line into the find bar"
    x11_end
    # A middle click on the field pastes PRIMARY (the xclip's, then VOMD's own).
    printf 'primary é\nsecond' | xclip -i -selection primary
    printf 'primary é' >primary.first
    x11_vomd small.md cfg-on
    x11_key ctrl+f
    sleep 0.3
    DISPLAY=:99 xdotool mousemove "$(field_x)" "$(field_y)"
    sleep 0.3
    DISPLAY=:99 xdotool click 2
    sleep 0.5
    x11_key ctrl+a ctrl+c
    retry x11_same clipboard primary.first || fail "X11: a middle click on the field: the query came out as '$(x11_paste clipboard | od -c | head -3)'"
    ok "X11: a middle click on the field pastes PRIMARY"
    x11_end
    # A selection VOMD owns itself: select all (PRIMARY), then Ctrl+C (CLIPBOARD), and the bar
    # takes it from VOMD's own thread.
    x11_vomd small.md cfg-on
    x11_key ctrl+a ctrl+c
    retry x11_same clipboard small.expected || fail "X11: Ctrl+C didn't copy"
    x11_key ctrl+f
    sleep 0.3
    x11_key ctrl+v
    sleep 0.5
    printf other | xclip -i -selection clipboard
    retry test "$(x11_paste clipboard)" = other || fail "X11: another program couldn't take the clipboard"
    x11_key ctrl+a ctrl+c
    retry x11_same clipboard first.expected || fail "X11: pasting VOMD's own clipboard: '$(x11_paste clipboard | od -c | head -3)'"
    ok "X11: the clipboard VOMD owns is pasted from what it offers"
    x11_end
    x11_vomd small.md cfg-on
    x11_key ctrl+a
    retry x11_same primary small.expected || fail "X11: Ctrl+A didn't offer the document as PRIMARY"
    x11_key ctrl+f
    sleep 0.3
    DISPLAY=:99 xdotool mousemove "$(field_x)" "$(field_y)"
    sleep 0.3
    DISPLAY=:99 xdotool click 2
    sleep 0.5
    x11_key ctrl+a ctrl+c
    retry x11_same clipboard first.expected || fail "X11: pasting VOMD's own PRIMARY: '$(x11_paste clipboard | od -c | head -3)'"
    ok "X11: the PRIMARY VOMD owns is pasted from what it offers"
    x11_end
    # The field's own selection and cut: Ctrl+A in it offers it as PRIMARY, Ctrl+X takes it
    # out and makes it the clipboard, and what is pasted next goes into the empty field.
    x11_vomd small.md cfg-on
    x11_key ctrl+f
    sleep 0.3
    DISPLAY=:99 xdotool type --delay 60 'hello world'
    sleep 0.3
    x11_key ctrl+a
    retry x11_same primary hello.expected || fail "X11: Ctrl+A in the field didn't offer it as PRIMARY: '$(x11_paste primary)'"
    x11_key ctrl+x
    retry x11_same clipboard hello.expected || fail "X11: Ctrl+X didn't cut to the clipboard: '$(x11_paste clipboard)'"
    printf OTHER | xclip -i -selection clipboard
    retry test "$(x11_paste clipboard)" = OTHER || fail "X11: another program couldn't take the clipboard"
    x11_key ctrl+v
    sleep 0.5
    x11_key ctrl+a ctrl+c
    retry x11_same clipboard other.expected || fail "X11: Ctrl+X left text in the field: '$(x11_paste clipboard)'"
    ok "X11: the field's selection is PRIMARY, Ctrl+X cuts it, Ctrl+V pastes into what is left"
    x11_end
    # The next checks start with no owner of either selection.
    pkill -x xclip 2>/dev/null || true
    retry x11_unset clipboard || fail "X11: the clipboard still has an owner"
    retry x11_unset primary || fail "X11: PRIMARY still has an owner"
}

x11_suite() {
    echo "== X11"
    export DISPLAY=:99
    x11_server
    x11_reading

    echo "-- short text"
    x11_vomd small.md cfg-on
    x11_key ctrl+a
    retry x11_same primary small.expected || fail "X11: Ctrl+A didn't offer the document as PRIMARY: $(x11_paste primary | od -c | head -5)"
    x11_unset clipboard || fail "X11: Ctrl+A alone set the clipboard"
    x11_key ctrl+c
    retry x11_same clipboard small.expected || fail "X11: Ctrl+C didn't copy: $(x11_paste clipboard | od -c | head -5)"
    ok "X11: Ctrl+A is PRIMARY, Ctrl+C is the clipboard"
    local targets
    targets=$(xclip -o -selection clipboard -t TARGETS)
    for t in TARGETS TIMESTAMP MULTIPLE UTF8_STRING 'text/plain;charset=utf-8' STRING TEXT; do
        grep -qxF "$t" <<<"$targets" || fail "X11: no target $t in: $targets"
    done
    xclip -o -selection clipboard -t STRING >got.txt
    cmp -s got.txt small.latin1 || fail "X11: STRING isn't Latin-1: $(od -c got.txt | head -3)"
    xsel -ob >got.txt
    cmp -s got.txt small.expected || fail "X11: xsel -ob differs"
    xsel -op >got.txt
    cmp -s got.txt small.expected || fail "X11: xsel -op differs"
    ok "X11: targets, Latin-1 STRING, xsel"

    echo "-- idle"
    sleep 2
    local before after
    before=$(ticks "$pid")
    sleep 3
    after=$(ticks "$pid")
    [ $((after - before)) -le 1 ] || fail "X11: the idle window used $((after - before)) ticks of CPU in 3 s"
    x11_same clipboard small.expected || fail "X11: no answer after 3 s idle"
    x11_same primary small.expected || fail "X11: no PRIMARY answer after 3 s idle"
    ok "X11: answers from idle, which cost no CPU while idle"

    echo "-- ownership"
    printf other | xclip -i -selection clipboard
    retry test "$(x11_paste clipboard)" = other || fail "X11: another program couldn't take the clipboard"
    sleep 1
    [ "$(x11_paste clipboard)" = other ] || fail "X11: VOMD answered for a selection it lost"
    x11_key ctrl+c
    retry x11_same clipboard small.expected || fail "X11: Ctrl+C didn't take the clipboard back"
    ok "X11: a selection another program takes is theirs, and Ctrl+C takes it back"

    echo "-- mouse"
    x11_pointer 30
    DISPLAY=:99 xdotool click --repeat 2 --delay 80 1
    retry x11_same primary word.expected || fail "X11: double-click: PRIMARY is '$(x11_paste primary)'"
    sleep 1
    DISPLAY=:99 xdotool click --repeat 3 --delay 80 1
    retry x11_same primary first.expected || fail "X11: triple-click: PRIMARY is '$(x11_paste primary)'"
    sleep 1
    x11_pointer 3
    DISPLAY=:99 xdotool mousedown 1
    x11_pointer 60
    x11_pointer 140
    DISPLAY=:99 xdotool mouseup 1
    sleep 1
    local dragged
    dragged=$(x11_paste primary)
    case "$(cat first.expected)" in *"$dragged"*) ;; *) fail "X11: the drag offered '$dragged'" ;; esac
    [ "${#dragged}" -ge 5 ] || fail "X11: the drag offered only '$dragged'"
    x11_same clipboard small.expected || fail "X11: selecting with the mouse changed the clipboard"
    ok "X11: double-click, triple-click and drag are PRIMARY; the clipboard is Ctrl+C's alone"
    x11_end

    echo "-- copy button"
    x11_vomd code.md cfg-on
    # Pointing at the block shows the button, and nothing is copied by that.
    printf keep | xclip -i -selection clipboard
    x11_pointer 30
    sleep 0.5
    DISPLAY=:99 xdotool mousemove "$(button_x)" "$(button_y)"
    sleep 0.5
    [ "$(x11_paste clipboard)" = keep ] || fail "X11: pointing at the copy button changed the clipboard"
    # A click on the code beside the button copies nothing, and a selection is made there.
    x11_pointer 30
    DISPLAY=:99 xdotool click 1
    sleep 0.7
    [ "$(x11_paste clipboard)" = keep ] || fail "X11: a click beside the copy button changed the clipboard"
    # Select all (PRIMARY is the whole page), then click the button.
    x11_key ctrl+a
    retry x11_same primary code.all || fail "X11: Ctrl+A didn't offer the page as PRIMARY: $(x11_paste primary | od -c | head -5)"
    DISPLAY=:99 xdotool mousemove "$(button_x)" "$(button_y)"
    sleep 0.3
    DISPLAY=:99 xdotool click 1
    retry x11_same clipboard code.expected || fail "X11: the copy button didn't copy the code exactly: $(x11_paste clipboard | od -c | head -8)"
    sleep 0.5
    x11_same primary code.all || fail "X11: the copy button changed PRIMARY: $(x11_paste primary | od -c | head -5)"
    xclip -o -selection clipboard -t STRING >got.txt
    cmp -s got.txt code.latin1 || fail "X11: the copied code's STRING differs: $(od -c got.txt | head -5)"
    ok "X11: a click on the copy button makes the clipboard exactly the code, and leaves PRIMARY alone"
    # Clicking it again copies again, whatever was copied in between.
    printf other | xclip -i -selection clipboard
    retry test "$(x11_paste clipboard)" = other || fail "X11: another program couldn't take the clipboard"
    sleep 2.5
    DISPLAY=:99 xdotool click 1
    retry x11_same clipboard code.expected || fail "X11: a second click didn't copy the code again"
    ok "X11: the button copies again"
    # Idle after the check mark's two seconds: the loop sleeps (no CPU).
    sleep 2.5
    before=$(ticks "$pid")
    sleep 3
    after=$(ticks "$pid")
    [ $((after - before)) -le 1 ] || fail "X11: the idle window with a button used $((after - before)) ticks of CPU in 3 s"
    ok "X11: after the check mark goes, the window is idle"
    x11_end

    echo "-- special requests"
    x11_vomd small.md cfg-on
    x11_key ctrl+a ctrl+c
    retry x11_same clipboard small.expected || fail "X11: Ctrl+C didn't copy"
    "$probe" multiple CLIPBOARD >multiple.out || fail "X11: MULTIPLE wasn't answered"
    grep -qx 'UTF8_STRING -> PROBE_1' multiple.out || fail "X11: MULTIPLE: $(cat multiple.out)"
    grep -qx 'STRING -> PROBE_2' multiple.out || fail "X11: MULTIPLE: $(cat multiple.out)"
    grep -qx 'NO_SUCH_TARGET -> None' multiple.out || fail "X11: MULTIPLE didn't refuse a target it has none of: $(cat multiple.out)"
    grep -qx 'Paragraph one: café 日本 and some more words.' multiple.out || fail "X11: MULTIPLE gave no text: $(cat multiple.out)"
    ok "X11: MULTIPLE answers what it can and says no to the rest"
    stamp=$("$probe" timestamp CLIPBOARD)
    [ "$stamp" -gt 0 ] || fail "X11: TIMESTAMP is '$stamp'"
    [ "$("$probe" early CLIPBOARD)" = refused ] || fail "X11: a request from before the selection was owned was answered"
    ok "X11: TIMESTAMP is the time of the claim, and an older request is refused"
    x11_end

    echo "-- clipboard manager at exit"
    x11_vomd small.md cfg-on
    x11_key ctrl+a ctrl+c
    retry x11_same clipboard small.expected || fail "X11: Ctrl+C didn't copy"
    rm -f manager.out
    "$probe" manager manager.out >manager.log &
    manager=$!
    pids+=("$manager")
    retry grep -qx ready manager.log || fail "X11: the test's clipboard manager didn't start"
    x11_key ctrl+w
    retry exited "$pid" || fail "X11: VOMD didn't quit"
    retry test -s manager.out || fail "X11: VOMD didn't ask the clipboard manager to save the clipboard"
    cmp -s manager.out small.expected || fail "X11: the manager saved: $(od -c manager.out | head -3)"
    wait "$manager" 2>/dev/null || true
    ok "X11: the clipboard manager was asked at exit, and got the text"
    # With no manager, quitting isn't held up.
    x11_vomd small.md cfg-on
    x11_key ctrl+a ctrl+c
    retry x11_same clipboard small.expected || fail "X11: Ctrl+C didn't copy"
    local t0 t1
    t0=$(date +%s%N)
    x11_key ctrl+w
    retry exited "$pid" || fail "X11: VOMD didn't quit"
    t1=$(date +%s%N)
    [ $(((t1 - t0) / 1000000)) -lt 800 ] || fail "X11: quitting took $(((t1 - t0) / 1000000)) ms with no clipboard manager"
    ok "X11: with no clipboard manager, quitting isn't delayed"

    echo "-- nothing selected"
    x11_vomd small.md cfg-on
    printf keep | xclip -i -selection clipboard
    sleep 0.5
    x11_key ctrl+c
    sleep 1
    [ "$(x11_paste clipboard)" = keep ] || fail "X11: Ctrl+C with nothing selected changed the clipboard"
    ok "X11: Ctrl+C with no selection leaves the clipboard alone"
    x11_end

    echo "-- 5 MB"
    x11_vomd big.md cfg-on
    sleep 2
    local rss0 rss1 rss2 peak
    rss0=$(rss_kb "$pid")
    x11_key ctrl+a
    x11_key ctrl+c
    retry x11_same clipboard big.expected || fail "X11: the 5 MB copy differs"
    rss1=$(rss_kb "$pid")
    local sums
    sums=$(xclip -o -selection clipboard | sha256sum | cut -d' ' -f1)
    [ "$sums" = "$big_sha" ] || fail "X11: INCR through xclip gave sha256 $sums"
    sums=$(xsel -ob | sha256sum | cut -d' ' -f1)
    [ "$sums" = "$big_sha" ] || fail "X11: INCR through xsel gave sha256 $sums"
    (xclip -o -selection clipboard | sha256sum | cut -d' ' -f1 >sum1) &
    local second
    second=$!
    xsel -ob | sha256sum | cut -d' ' -f1 >sum2
    wait "$second"
    [ "$(cat sum1)" = "$big_sha" ] && [ "$(cat sum2)" = "$big_sha" ] || fail "X11: two pastes at once: $(cat sum1) $(cat sum2)"
    xclip -o -selection primary | sha256sum | cut -d' ' -f1 | grep -qx "$big_sha" || fail "X11: the 5 MB PRIMARY differs"
    rss2=$(rss_kb "$pid")
    peak=$(peak_kb "$pid")
    echo "   VOMD's memory: $rss0 KB before, $rss1 KB after the copy, $rss2 KB after four pastes (peak $peak KB)"
    [ $((rss1 - rss0)) -lt 3000 ] || fail "X11: copying took $((rss1 - rss0)) KB, the text is 5300 KB"
    [ $((rss2 - rss0)) -lt 3000 ] || fail "X11: pasting took $((rss2 - rss0)) KB"
    ok "X11: 5 MB through INCR, to xclip and xsel, two at a time, and PRIMARY, without holding the text"
    x11_end

    echo "-- primary_selection = off"
    x11_vomd small.md cfg-off
    x11_key ctrl+a
    sleep 1
    x11_unset primary || fail "X11: Ctrl+A set PRIMARY with primary_selection = off"
    x11_pointer 30
    DISPLAY=:99 xdotool click --repeat 2 --delay 80 1
    sleep 1
    x11_unset primary || fail "X11: a double-click set PRIMARY with primary_selection = off"
    x11_key ctrl+a ctrl+c
    retry x11_same clipboard small.expected || fail "X11: Ctrl+C doesn't work with primary_selection = off"
    x11_unset primary || fail "X11: Ctrl+C set PRIMARY"
    ok "X11: primary_selection = off leaves PRIMARY alone, and Ctrl+C works"
    x11_end
}

# --- Wayland -----------------------------------------------------------------------------------

sway_ipc() { XDG_RUNTIME_DIR=/run/sway swaymsg -s /run/sway/sway-ipc.*.sock "$@"; }

wayland_server() {
    mkdir -p /run/sway
    chmod 700 /run/sway
    printf 'output HEADLESS-1 resolution 1280x900\ndefault_border none\nxwayland disable\n' >sway.conf
    XDG_RUNTIME_DIR=/run/sway WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman \
        sway -c sway.conf >sway.log 2>&1 &
    pids+=($!)
    local _
    for _ in $(seq 100); do
        ls /run/sway/sway-ipc.*.sock >/dev/null 2>&1 && ls /run/sway/wayland-[0-9]* >/dev/null 2>&1 && break
        sleep 0.2
    done
    sway_display=$(ls /run/sway | grep -E '^wayland-[0-9]+$' | head -1) || true
    [ -n "$sway_display" ] || fail "sway didn't start: $(cat sway.log)"
}

wl() { # command...: with only sway reachable
    env XDG_RUNTIME_DIR=/run/sway WAYLAND_DISPLAY="$sway_display" "$@"
}

wayland_vomd() { # file config
    env -u DISPLAY XDG_RUNTIME_DIR=/run/sway WAYLAND_DISPLAY="$sway_display" \
        XDG_CONFIG_HOME="$work/$2" HOME="$work/home" "$vomd" "$1" >vomd.log 2>&1 &
    pid=$!
    pids+=("$pid")
    local _
    for _ in $(seq 100); do
        sway_ipc -t get_tree | grep -q "\"name\": \"$1\"" && break
        sleep 0.1
    done
    sway_ipc -t get_tree | grep -q "\"name\": \"$1\"" || fail "no VOMD window for $1: $(cat vomd.log)"
    sleep 1.5
}

wayland_end() {
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    retry bash -c '! XDG_RUNTIME_DIR=/run/sway swaymsg -s /run/sway/sway-ipc.*.sock -t get_tree | grep -q "\"app_id\": \"vomd\""' || true
}

# A virtual keyboard that is made and used at once isn't known to a client yet, so the first key
# waits (and is sent again if it was lost: `until` says whether it got through).
wl_keys() { wl wtype -s 400 "$@"; }

wl_paste() { wl wl-paste -n "$@" 2>/dev/null; }
wl_same() { # file, wl-paste options...
    local file=$1
    shift
    wl_paste "$@" >got.txt && cmp -s got.txt "$file"
}
wl_unset() { # options
    ! wl wl-paste -n "$@" >got.txt 2>/dev/null && [ ! -s got.txt ]
}

# What the probe's window reads from the selection `$1` with VOMD's code (`$2`: it offers that
# first, itself): the text it prints, with nothing printed (and a failure) if there was none. A
# headless sway has no keyboard, and a window is only told of the selections when it has one, so
# a virtual keyboard is held (Shift down, for as long as the probe takes) while it runs.
wl_read() {
    # (`env` directly, not through `wl`: a function in the background is a subshell, and killing
    # it left wtype holding its virtual keyboard for the 20 s, which sway kept sending VOMD's
    # window keymaps for, so the idle window used CPU.)
    env XDG_RUNTIME_DIR=/run/sway WAYLAND_DISPLAY="$sway_display" wtype -M shift -s 20000 -m shift &
    local keyboard=$! status=0
    sleep 0.3
    wl timeout 30 "$probe" read "$@" || status=$?
    kill "$keyboard" 2>/dev/null || true
    wait "$keyboard" 2>/dev/null || true
    return "$status"
}

wayland_reading() {
    echo "-- reading"
    local keys
    wl_read PRIMARY >got.txt 2>err.txt && fail "Wayland: read a PRIMARY that has no owner: $(cat got.txt)"
    grep -qx nothing err.txt || fail "Wayland: no owner: $(cat err.txt)"
    ok "Wayland: a selection with no owner reads as nothing"
    wl wl-copy <offer.txt
    wl_read CLIPBOARD >got.txt || fail "Wayland: reading CLIPBOARD: $(cat err.txt)"
    cmp -s got.txt offer.txt || fail "Wayland: CLIPBOARD read as: $(od -c got.txt | head -5)"
    wl wl-copy --primary <offer.txt
    wl_read PRIMARY >got.txt || fail "Wayland: reading PRIMARY: $(cat err.txt)"
    cmp -s got.txt offer.txt || fail "Wayland: PRIMARY read as: $(od -c got.txt | head -5)"
    ok "Wayland: VOMD's code reads CLIPBOARD and PRIMARY that wl-copy offers"
    # (wl-copy gives the bytes it is given for whatever text type it offers, so a Latin-1 `STRING`
    # can't be had from it as it can from xclip; a unit test covers that type. This is a type with
    # no charset, the last that is tried.)
    wl wl-copy -t text/plain <offer.txt
    wl_read CLIPBOARD >got.txt || fail "Wayland: reading a text/plain clipboard"
    cmp -s got.txt offer.txt || fail "Wayland: text/plain read as: $(od -c got.txt | head -3)"
    ok "Wayland: a clipboard that offers text/plain is read"
    wl wl-copy <big.expected
    wl_read CLIPBOARD >got.txt || fail "Wayland: reading a 5 MB clipboard"
    cmp -s got.txt big.cap || fail "Wayland: the 5 MB clipboard read as $(stat -c %s got.txt) bytes"
    wl wl-copy --primary <big.expected
    wl_read PRIMARY >got.txt || fail "Wayland: reading a 5 MB PRIMARY"
    cmp -s got.txt big.cap || fail "Wayland: the 5 MB PRIMARY read as $(stat -c %s got.txt) bytes"
    ok "Wayland: a 5 MB clipboard and PRIMARY are read up to 64 KiB"
    wl wl-copy -t image/png <small.md
    wl_read CLIPBOARD >got.txt 2>err.txt && fail "Wayland: read an image offer as text: $(cat got.txt)"
    grep -qx nothing err.txt || fail "Wayland: an offer with no text: $(cat err.txt)"
    wl wl-copy --clear
    wl_read CLIPBOARD >got.txt 2>err.txt && fail "Wayland: read a cleared clipboard: $(cat got.txt)"
    ok "Wayland: an offer with no text, and a cleared clipboard, read as nothing"
    wl_read CLIPBOARD 'own text é' >got.txt || fail "Wayland: reading its own selection: $(cat err.txt)"
    [ "$(cat got.txt)" = 'own text é' ] || fail "Wayland: its own selection read as '$(cat got.txt)'"
    wl_read PRIMARY 'own primary é' >got.txt || fail "Wayland: reading its own PRIMARY: $(cat err.txt)"
    [ "$(cat got.txt)" = 'own primary é' ] || fail "Wayland: its own PRIMARY read as '$(cat got.txt)'"
    ok "Wayland: a selection VOMD's code offers itself reads back from what it offers"

    echo "-- pasting into the find bar"
    # (Prime the virtual keyboard with a key that does nothing, then send the keys at once.)
    keys=(-M ctrl f -m ctrl -M ctrl v -m ctrl -M ctrl a -m ctrl -M ctrl c -m ctrl)
    wl wl-copy <offer.txt
    wayland_vomd small.md cfg-on
    for _ in 1 2; do wl_keys -k Shift_L; done
    wl_keys "${keys[@]}"
    retry wl_same offer.first || fail "Wayland: Ctrl+V into the find bar: the query came out as '$(wl_paste | od -c | head -3)'"
    ok "Wayland: Ctrl+V pastes the clipboard's first line into the find bar"
    wayland_end
    # VOMD's own clipboard, which the bar takes from what VOMD offers.
    wayland_vomd small.md cfg-on
    for _ in 1 2; do wl_keys -k Shift_L; done
    wl_keys -M ctrl a -m ctrl -M ctrl c -m ctrl
    retry wl_same small.expected || fail "Wayland: Ctrl+C didn't copy"
    wl_keys -M ctrl f -m ctrl -M ctrl v -m ctrl
    sleep 0.5
    printf other | wl wl-copy
    retry test "$(wl_paste)" = other || fail "Wayland: another program couldn't take the clipboard"
    wl_keys -M ctrl a -m ctrl -M ctrl c -m ctrl
    retry wl_same first.expected || fail "Wayland: pasting VOMD's own clipboard: '$(wl_paste | od -c | head -3)'"
    ok "Wayland: the clipboard VOMD owns is pasted from what it offers"
    wayland_end
    # The field's own selection (PRIMARY), cut and paste.
    wayland_vomd small.md cfg-on
    for _ in 1 2; do wl_keys -k Shift_L; done
    wl_keys -M ctrl f -m ctrl
    wl_keys hello world
    wl_keys -M ctrl a -m ctrl
    retry wl_same hello.expected --primary || fail "Wayland: Ctrl+A in the field didn't offer it as PRIMARY: '$(wl_paste --primary)'"
    wl_keys -M ctrl x -m ctrl
    retry wl_same hello.expected || fail "Wayland: Ctrl+X didn't cut to the clipboard: '$(wl_paste)'"
    printf OTHER | wl wl-copy
    retry test "$(wl_paste)" = OTHER || fail "Wayland: another program couldn't take the clipboard"
    wl_keys -M ctrl v -m ctrl
    sleep 0.5
    wl_keys -M ctrl a -m ctrl -M ctrl c -m ctrl
    retry wl_same other.expected || fail "Wayland: Ctrl+X left text in the field: '$(wl_paste)'"
    ok "Wayland: the field's selection is PRIMARY, Ctrl+X cuts it, Ctrl+V pastes into what is left"
    wayland_end
    wl wl-copy --clear
    wl wl-copy --clear --primary
}

wayland_suite() {
    echo "== Wayland (sway)"
    unset DISPLAY
    wayland_server
    wayland_reading

    echo "-- short text"
    wayland_vomd small.md cfg-on
    local _
    for _ in 1 2 3; do
        wl_keys -M ctrl a -m ctrl
        retry wl_same small.expected --primary && break
    done
    wl_same small.expected --primary || fail "Wayland: Ctrl+A didn't offer the document as PRIMARY: $(wl_paste --primary | od -c | head -5)"
    wl_unset || fail "Wayland: Ctrl+A alone set the clipboard"
    wl_keys -M ctrl c -m ctrl
    retry wl_same small.expected || fail "Wayland: Ctrl+C didn't copy: $(wl_paste | od -c | head -5)"
    ok "Wayland: Ctrl+A is PRIMARY, Ctrl+C is the clipboard"
    local types
    types=$(wl wl-paste -l)
    for t in 'text/plain;charset=utf-8' UTF8_STRING TEXT STRING text/plain; do
        grep -qxF "$t" <<<"$types" || fail "Wayland: no type $t in: $types"
    done
    wl wl-paste -n -t STRING >got.txt
    cmp -s got.txt small.latin1 || fail "Wayland: STRING isn't Latin-1: $(od -c got.txt | head -3)"
    wl_same small.expected -t UTF8_STRING || fail "Wayland: UTF8_STRING differs"
    ok "Wayland: types, Latin-1 STRING"

    echo "-- idle"
    # No virtual keyboard may be left: sway sends the window its keymap again for each, and the
    # window's work on those is no idleness of its own.
    local stray
    stray=$(ps -eo stat=,pid=,args= | awk '$1 !~ /^Z/ && $3 == "wtype"')
    [ -z "$stray" ] || fail "Wayland: a wtype is still running: $stray"
    sleep 2
    local before after
    before=$(ticks "$pid")
    sleep 3
    after=$(ticks "$pid")
    [ $((after - before)) -le 1 ] || fail "Wayland: the idle window used $((after - before)) ticks of CPU in 3 s"
    wl_same small.expected || fail "Wayland: no answer after 3 s idle"
    wl_same small.expected --primary || fail "Wayland: no PRIMARY answer after 3 s idle"
    ok "Wayland: answers from idle, which cost no CPU while idle"

    echo "-- ownership"
    printf other | wl wl-copy
    retry test "$(wl_paste)" = other || fail "Wayland: another program couldn't take the clipboard"
    sleep 1
    [ "$(wl_paste)" = other ] || fail "Wayland: VOMD answered for a selection it lost"
    wl_keys -M ctrl c -m ctrl
    retry wl_same small.expected || fail "Wayland: Ctrl+C didn't take the clipboard back"
    ok "Wayland: a selection another program takes is theirs, and Ctrl+C takes it back"

    echo "-- nothing selected"
    wayland_vomd small.md cfg-on
    printf keep | wl wl-copy
    sleep 0.5
    wl_keys -M ctrl c -m ctrl
    sleep 1
    [ "$(wl_paste)" = keep ] || fail "Wayland: Ctrl+C with nothing selected changed the clipboard"
    ok "Wayland: Ctrl+C with no selection leaves the clipboard alone"
    wayland_end

    echo "-- 5 MB"
    wayland_vomd big.md cfg-on
    sleep 2
    local rss0 rss1 rss2 peak
    rss0=$(rss_kb "$pid")
    wl_keys -M ctrl a -m ctrl
    sleep 0.3
    wl_keys -M ctrl c -m ctrl
    retry wl_same big.expected || fail "Wayland: the 5 MB copy differs"
    rss1=$(rss_kb "$pid")
    local sums
    sums=$(wl wl-paste -n | sha256sum | cut -d' ' -f1)
    [ "$sums" = "$big_sha" ] || fail "Wayland: the pipe gave sha256 $sums"
    (wl wl-paste -n | sha256sum | cut -d' ' -f1 >sum1) &
    local second
    second=$!
    wl wl-paste -n | sha256sum | cut -d' ' -f1 >sum2
    wait "$second"
    [ "$(cat sum1)" = "$big_sha" ] && [ "$(cat sum2)" = "$big_sha" ] || fail "Wayland: two pastes at once: $(cat sum1) $(cat sum2)"
    wl wl-paste -n --primary | sha256sum | cut -d' ' -f1 | grep -qx "$big_sha" || fail "Wayland: the 5 MB PRIMARY differs"
    rss2=$(rss_kb "$pid")
    peak=$(peak_kb "$pid")
    echo "   VOMD's memory: $rss0 KB before, $rss1 KB after the copy, $rss2 KB after four pastes (peak $peak KB)"
    [ $((rss1 - rss0)) -lt 3000 ] || fail "Wayland: copying took $((rss1 - rss0)) KB, the text is 5300 KB"
    [ $((rss2 - rss0)) -lt 3000 ] || fail "Wayland: pasting took $((rss2 - rss0)) KB"
    ok "Wayland: 5 MB through the pipe, two at a time, and PRIMARY, without holding the text"
    wayland_end

    echo "-- primary_selection = off"
    wayland_vomd small.md cfg-off
    for _ in 1 2 3; do
        wl_keys -M ctrl a -m ctrl
        sleep 0.5
        # (The first keys may be lost: try the clipboard to learn that VOMD got them.)
        wl_keys -M ctrl c -m ctrl
        retry wl_same small.expected && break
    done
    wl_same small.expected || fail "Wayland: Ctrl+C doesn't work with primary_selection = off"
    wl_unset --primary || fail "Wayland: Ctrl+A set PRIMARY with primary_selection = off"
    ok "Wayland: primary_selection = off leaves PRIMARY alone, and Ctrl+C works"
    wayland_end
}

for suite in $suites; do
    "${suite}_suite"
done
echo "== all clipboard checks passed"
