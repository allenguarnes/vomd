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
#   - the answers come after 3 s of idle, and the idle window used no CPU for them;
#   - another program taking the selection ends VOMD's offer, and a new Ctrl+C gets it back;
#   - a 5 MB document copies whole (X11's INCR, Wayland's pipe), byte for byte, to two programs
#     at once, without VOMD's memory growing by the text;
#   - with `primary_selection = off`, PRIMARY is never set, and the clipboard still works;
#   - (X11 only) the rarer requests: MULTIPLE, TIMESTAMP, and one from before VOMD owned the
#     selection (refused), made by `examples/clip_probe.rs`; and at exit a clipboard manager
#     (also the probe) is asked to save the clipboard, and does, while with none VOMD quits at
#     once.
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
awk -v n=90000 'BEGIN { for (i = 1; i <= n; i++) { if (i > 1) printf "\n\n"; printf "Paragraph %d of the big document, with a little text in it.", i } }' >big.expected
{ cat big.expected; echo; } >big.md
big_sha=$(sha256sum <big.expected | cut -d' ' -f1)
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

# `paste sel`: what a program gets from the selection (clipboard or primary); `same sel file`.
x11_paste() { xclip -o -selection "$1" 2>/dev/null; }
x11_same() { # sel, file
    x11_paste "$1" >got.txt && cmp -s got.txt "$2"
}
x11_unset() { ! xclip -o -selection "$1" >got.txt 2>/dev/null && [ ! -s got.txt ]; }

x11_suite() {
    echo "== X11"
    export DISPLAY=:99
    x11_server

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

wayland_suite() {
    echo "== Wayland (sway)"
    unset DISPLAY
    wayland_server

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
