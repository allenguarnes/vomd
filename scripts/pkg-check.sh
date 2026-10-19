#!/usr/bin/env bash
# Check dist/'s Debian package and AppImage (`cargo xtask dist`, or scripts/pkg-build.sh on a
# host with a newer glibc) in clean containers of every supported distribution. Needs docker.
#
# For each image: the .deb is installed with apt, which brings only what it depends on; VOMD
# then has to show its first frame on an X server (Xvfb) and on a Wayland compositor (weston,
# headless) that run in containers of their own, so their libraries can't stand in for a
# missing dependency; a screenshot must show a page, not just VOMD's background; lintian must
# find no errors; removal must leave nothing. The AppImage is run in another clean container
# with only the display libraries, through --appimage-extract-and-run (containers have no FUSE).
# For both, Ctrl+O in the empty window must open the file a (stand-in) file dialog gives.
# Screenshots land in the printed work dir. VOMD_PKG_IMAGES overrides the images.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
images=${VOMD_PKG_IMAGES:-"debian:12 debian:13 ubuntu:24.04"}
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
deb=$(ls "$root"/dist/vomd_"$ver"-*_amd64.deb 2>/dev/null | tail -1 || true)
appimage="$root/dist/vomd-$ver-x86_64.AppImage"
[ -f "$deb" ] || { echo "FAIL: no dist/vomd_$ver-*_amd64.deb: run cargo xtask dist"; exit 1; }
[ -f "$appimage" ] || { echo "FAIL: no $appimage: run cargo xtask dist"; exit 1; }
# The libraries the .deb depends on, which the AppImage needs from the system as well.
libs=$(dpkg-deb -f "$deb" Depends 2>/dev/null || ar p "$deb" control.tar.xz | tar xJO ./control |
    sed -n 's/^Depends: //p')
libs=$(echo "$libs" | tr ',' '\n' | sed 's/(.*)//; s/ //g' | grep -v '^libc6$' | tr '\n' ' ')
work=$(mktemp -d /tmp/vomd-pkg-check.XXXXXX)
mkdir -p "$work/x11" "$work/wayland" "$work/shots" "$work/pkg"
chmod 700 "$work/wayland"
cp "$deb" "$work/pkg/vomd.deb"
cp "$appimage" "$work/pkg/vomd.AppImage"
echo "== work dir: $work"
echo "== $(basename "$deb"), $(basename "$appimage")"

label=vomd-pkg-check=$$
cleanup() {
    docker ps -aq --filter "label=$label" | xargs -r docker rm -f >/dev/null 2>&1 || true
    # Files the containers made as root.
    docker run --rm -v "$work:/w" debian:12 sh -c 'rm -rf /w/x11 /w/wayland' >/dev/null 2>&1 || true
}
trap cleanup EXIT

start() { # image, extra docker args...
    local image=$1
    shift
    local c
    c=$(docker run -d --rm --label "$label" "$@" \
        -v "$work/x11:/tmp/.X11-unix" -v "$work/wayland:/run/wayland" \
        -v "$work/pkg:/pkg:ro" -v "$root/fixtures:/fixtures:ro" \
        -e DISPLAY=:99 -e XDG_RUNTIME_DIR=/run/wayland -e DEBIAN_FRONTEND=noninteractive -e DEBCONF_NOWARNINGS=yes \
        "$image" sleep infinity)
    echo "$c"
}
dx() { docker exec "$@"; }
apt_install() { # container, packages...
    local c=$1
    shift
    dx "$c" sh -c "apt-get update -qq && apt-get install -y -qq --no-install-recommends $* >/dev/null"
}
fail() {
    echo "FAIL: $*"
    exit 1
}

# The display servers.
echo "== display servers"
server=$(start debian:13)
apt_install "$server" xvfb weston imagemagick x11-utils xdotool
dx -d "$server" Xvfb :99 -screen 0 1280x900x24 -nolisten tcp -ac
dx -d "$server" weston --backend=headless --renderer=pixman --socket=wayland-vomd --width=1280 \
    --height=900 --idle-time=0
for _ in $(seq 50); do
    [ -S "$work/x11/X99" ] && [ -S "$work/wayland/wayland-vomd" ] && break
    sleep 0.2
done
[ -S "$work/x11/X99" ] || fail "Xvfb didn't start"
[ -S "$work/wayland/wayland-vomd" ] || fail "weston didn't start"

# First frame on X11 and on Wayland: VOMD_BENCH=1 prints `vomd-bench first_frame_ms=…` once a
# frame is presented, and exits. Each run can reach only its own server: no WAYLAND_DISPLAY (and
# no wayland-0) for X11, an empty DISPLAY for Wayland, so neither falls back to the other.
first_frames() { # container, command...
    local c=$1
    shift
    local out
    out=$(dx -e VOMD_BENCH=1 "$c" timeout 30 "$@" --no-settings \
        /fixtures/readme.md 2>&1) || true
    grep -q 'vomd-bench first_frame_ms' <<<"$out" || fail "no first frame on X11: $out"
    out=$(dx -e VOMD_BENCH=1 -e WAYLAND_DISPLAY=wayland-vomd -e DISPLAY= "$c" timeout 30 "$@" \
        --no-settings /fixtures/readme.md 2>&1) || true
    grep -q 'vomd-bench first_frame_ms' <<<"$out" || fail "no first frame on Wayland: $out"
}

# A screenshot of the X server while VOMD shows golden.md: the page must not be blank. Only the
# window's page area counts (its geometry comes from the X server, below the toolbar strip): the
# window against the black root would pass any threshold on its own.
toolbar=32
screenshot() { # container, name, command...
    local c=$1 name=$2
    shift 2
    dx -d "$c" timeout 8 "$@" --no-settings /fixtures/golden.md
    sleep 4
    page "$name"
    sleep 4
}

# The X server's screen saved as `name`, and the page of the one VOMD window on it checked.
page() { # name
    local name=$1
    dx "$server" import -window root "/tmp/$name.png"
    docker cp -q "$server:/tmp/$name.png" "$work/shots/$name.png"
    local geo w h x y spread
    # The root's one child, `0x… "golden.md": ("vomd" "vomd")  900x810+0+0  +0+0`.
    geo=$(dx "$server" xwininfo -root -tree | grep -oE ' [0-9]+x[0-9]+\+[0-9]+\+[0-9]+ ' | head -1) || true
    [ -n "$geo" ] || fail "$name: no VOMD window on the X server"
    IFS='x+ ' read -r w h x y <<<"$geo"
    [ "$h" -gt "$toolbar" ] || fail "$name: window too small ($geo)"
    spread=$(dx "$server" convert "/tmp/$name.png" -crop "${w}x$((h - toolbar))+$x+$((y + toolbar))" \
        +repage -format '%[fx:standard_deviation]' info:)
    awk -v s="$spread" 'BEGIN { exit !(s > 0.05) }' || fail "$name: blank page ($spread)"
}

# Open: Ctrl+O in the empty window (`vomd` with no file) must open the file the dialog gives.
# The containers have no desktop portal, so VOMD falls back to zenity: here a stand-in that
# answers with golden.md. The window then has to be titled after it and show its page.
open_check() { # container, name, command...
    local c=$1 name=$2
    shift 2
    dx "$c" sh -c 'printf "#!/bin/sh\necho /fixtures/golden.md\n" > /usr/local/bin/zenity &&
        chmod +x /usr/local/bin/zenity'
    dx -d "$c" timeout 30 "$@" --no-settings
    local id="" _
    for _ in $(seq 75); do
        id=$(dx "$server" xdotool search --name '^vomd$' 2>/dev/null | head -1) || true
        [ -n "$id" ] && break
        sleep 0.2
    done
    [ -n "$id" ] || fail "$name: no empty window"
    sleep 1
    dx "$server" xdotool windowfocus --sync "$id" key ctrl+o
    local opened=""
    for _ in $(seq 75); do
        dx "$server" xdotool search --name '^golden\.md$' >/dev/null 2>&1 && opened=1 && break
        sleep 0.2
    done
    [ -n "$opened" ] || fail "$name: Ctrl+O didn't open the file the dialog gave"
    sleep 2
    page "$name"
    # End this VOMD (the AppImage's process is AppRun) and wait for its window to go, so the
    # next check finds only its own.
    dx "$c" sh -c 'for p in /proc/[0-9]*; do
        case "$(cat "$p/comm" 2>/dev/null)" in vomd | AppRun) kill "${p#/proc/}" ;; esac
    done; rm -f /usr/local/bin/zenity'
    for _ in $(seq 50); do
        dx "$server" xdotool search --name '.' >/dev/null 2>&1 || break
        sleep 0.2
    done
}

for image in $images; do
    tag=${image//[:\/]/-}
    echo "== $image: .deb"
    c=$(start "$image")
    apt_install "$c" /pkg/vomd.deb
    dx "$c" dpkg -s vomd | grep -q '^Status: install ok installed' || fail "not installed"
    dx "$c" vomd --version
    first_frames "$c" vomd
    screenshot "$c" "$tag-deb" vomd
    echo "   first frame on X11 and Wayland, screenshot $work/shots/$tag-deb.png"
    open_check "$c" "$tag-deb-open" vomd
    echo "   Ctrl+O opened the chosen file, screenshot $work/shots/$tag-deb-open.png"
    # The file dialogs Open uses are recommended (the install above left recommends out).
    dx "$c" dpkg-deb -f /pkg/vomd.deb Recommends | grep -q 'xdg-desktop-portal-gtk | zenity | kdialog' ||
        fail "the .deb doesn't recommend a file dialog"
    apt_install "$c" lintian
    # Lintian errors fail, and so does lintian failing to run: its exit status decides. Warnings
    # are printed.
    lint_status=0
    lint=$(dx "$c" lintian --fail-on error --tag-display-limit 0 --suppress-tags initial-upload-closes-no-bugs /pkg/vomd.deb 2>&1) || lint_status=$?
    [ -n "$lint" ] && echo "$lint" | sed 's/^/   /'
    [ "$lint_status" -eq 0 ] || fail "lintian exited with status $lint_status"
    files=$(dx "$c" sh -c 'dpkg -L vomd | while read -r f; do if [ -f "$f" ] || [ -L "$f" ]; then echo "$f"; fi; done')
    [ -n "$files" ] || fail "dpkg -L vomd lists no files"
    dx "$c" apt-get remove -y -qq vomd >/dev/null
    dx "$c" dpkg -s vomd 2>/dev/null | grep -q '^Status: install ok installed' && fail "still installed"
    # shellcheck disable=SC2086
    left=$(dx "$c" sh -c 'for f; do if [ -e "$f" ] || [ -L "$f" ]; then echo "$f"; fi; done' sh $files)
    [ -z "$left" ] || fail "left after removal: $left"
    docker rm -f "$c" >/dev/null

    echo "== $image: AppImage"
    c=$(start "$image")
    # shellcheck disable=SC2086
    apt_install "$c" $libs
    dx "$c" sh -c 'cp /pkg/vomd.AppImage /tmp/ && cd /tmp && ./vomd.AppImage --appimage-extract >/dev/null'
    for f in AppRun vomd.desktop vomd.svg .DirIcon usr/bin/vomd; do
        dx "$c" test -e "/tmp/squashfs-root/$f" || fail "AppImage lacks $f"
    done
    dx -e APPIMAGE_EXTRACT_AND_RUN=1 "$c" /tmp/vomd.AppImage --version
    first_frames "$c" env APPIMAGE_EXTRACT_AND_RUN=1 /tmp/vomd.AppImage
    screenshot "$c" "$tag-appimage" env APPIMAGE_EXTRACT_AND_RUN=1 /tmp/vomd.AppImage
    echo "   first frame on X11 and Wayland, screenshot $work/shots/$tag-appimage.png"
    open_check "$c" "$tag-appimage-open" env APPIMAGE_EXTRACT_AND_RUN=1 /tmp/vomd.AppImage
    echo "   Ctrl+O opened the chosen file, screenshot $work/shots/$tag-appimage-open.png"
    apt_install "$c" desktop-file-utils
    dx "$c" desktop-file-validate /tmp/squashfs-root/vomd.desktop
    docker rm -f "$c" >/dev/null
done
echo "== all packages pass"
