#!/usr/bin/env bash
# Check the clipboard against real display servers, in a container: Xvfb for X11 and a headless
# sway for Wayland. Needs docker. Nothing touches the desktop this runs on (no clipboard, no
# session): the servers, the window and the programs that paste are all in the container.
#
# VOMD is built as the release is, in a Debian 12 container (the toolchain and crates are
# the ones of the host's rustup and /tmp, as in pkg-build.sh), then run in a Debian 13 one
# with Xvfb, sway, xclip, xsel, wl-clipboard, xdotool and wtype, where scripts/clipboard-
# check-inside.sh drives it with real key and mouse events and pastes with the real programs:
# CLIPBOARD and PRIMARY, short and long (X11's INCR, Wayland's pipe), Latin-1 `STRING`, another
# program taking the selection away, `primary_selection = off`, a window with no seat. See the
# header of that script for what is checked.
#
# VOMD_CLIP_IMAGE overrides the image the servers run in (debian:13); VOMD_CLIP_BIN is a directory
# with the `vomd` binary and the `clip_probe` example to use instead of building them (they must
# run in that image).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
image=${VOMD_CLIP_IMAGE:-debian:13}
build_image=vomd-pkg-build-debian12
cache=/tmp/vomd-clip-cache
work=$(mktemp -d /tmp/vomd-clip-check.XXXXXX)
mkdir -p "$work/bin" "$cache/build" /tmp/vomd-pkg-cache/cargo

label=vomd-clip-check=$$
cleanup() {
    docker ps -aq --filter "label=$label" | xargs -r docker rm -f >/dev/null 2>&1 || true
    rm -rf "$work"
}
trap cleanup EXIT

if [ -n "${VOMD_CLIP_BIN:-}" ]; then
    cp "$VOMD_CLIP_BIN/vomd" "$VOMD_CLIP_BIN/clip_probe" "$work/bin/"
else
    channel=$(sed -n 's/^channel = "\(.*\)"/\1/p' "$root/rust-toolchain.toml")
    toolchain="${RUSTUP_HOME:-$HOME/.rustup}/toolchains/$channel-x86_64-unknown-linux-gnu"
    test -x "$toolchain/bin/cargo" || { echo "no $toolchain: rustup toolchain install $channel"; exit 1; }
    if ! docker image inspect "$build_image" >/dev/null 2>&1; then
        docker build -t "$build_image" - <<'EOF'
FROM debian:12
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    gcc libc6-dev binutils squashfs-tools xz-utils curl ca-certificates git \
 && rm -rf /var/lib/apt/lists/*
EOF
    fi
    echo "== building VOMD in Debian 12"
    docker run --rm --label "$label" -u "$(id -u):$(id -g)" \
        -v "$root:/src:ro" -v "$toolchain:/toolchain:ro" \
        -v /tmp/vomd-pkg-cache/cargo:/cargo -v "$cache/build:/build" -v "$work/bin:/out" \
        -e HOME=/tmp -e CARGO_HOME=/cargo -e PATH=/toolchain/bin:/usr/bin:/bin \
        "$build_image" sh -euc '
            find /build -mindepth 1 -maxdepth 1 ! -name target -exec rm -rf {} +
            tar -C /src --exclude=./target --exclude=./dist --exclude=./.git --exclude=./fuzz/target --exclude=./fuzz/corpus --exclude=./fuzz/artifacts -cf - . | tar -C /build -xf -
            cd /build
            cargo build --release --locked --bin vomd --example clip_probe
            cp target/release/vomd target/release/examples/clip_probe /out/'
fi
ls -l "$work/bin"

echo "== starting the servers' container ($image)"
c=$(docker run -d --label "$label" \
    -v "$work/bin:/vomd:ro" -v "$root/scripts/clipboard-check-inside.sh:/check.sh:ro" \
    -e DEBIAN_FRONTEND=noninteractive "$image" sleep infinity)
docker exec "$c" sh -c 'apt-get update -qq && apt-get install -y -qq --no-install-recommends \
    xvfb xauth xclip xsel xdotool sway wl-clipboard wtype procps coreutils gawk \
    libwayland-client0 libxkbcommon0 libxkbcommon-x11-0 libx11-6 libx11-xcb1 libxcb1 \
    libxcursor1 libxi6 >/dev/null'
docker exec -e VOMD_CLIPBOARD_CHECK=in-container -e VOMD_BIN=/vomd/vomd "$c" bash /check.sh "$@"
echo "== the clipboard works on X11 and Wayland"
