#!/usr/bin/env bash
# Build the release files (`cargo xtask dist --release`: tarball, .deb, AppImage) in a Debian 12
# container, as CI does: glibc 2.36, the oldest supported, so the binary names no newer glibc
# symbol, not even a weak one (an older loader warns about those at every start). Uses the
# host's rustup toolchain read-only; crates and build output are cached under /tmp, never in
# ~/.cargo. Needs docker. The files land in dist/.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
channel=$(sed -n 's/^channel = "\(.*\)"/\1/p' "$root/rust-toolchain.toml")
toolchain="${RUSTUP_HOME:-$HOME/.rustup}/toolchains/$channel-x86_64-unknown-linux-gnu"
test -x "$toolchain/bin/cargo" || { echo "no $toolchain: rustup toolchain install $channel"; exit 1; }
image=vomd-pkg-build-debian12
cache=/tmp/vomd-pkg-cache
mkdir -p "$cache/cargo" "$cache/build" "$root/dist"

if ! docker image inspect "$image" >/dev/null 2>&1; then
    docker build -t "$image" - <<'EOF'
FROM debian:12
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    gcc libc6-dev binutils squashfs-tools xz-utils curl ca-certificates git \
 && rm -rf /var/lib/apt/lists/*
EOF
fi

epoch=$(git -C "$root" log -1 --format=%ct)
docker run --rm -u "$(id -u):$(id -g)" \
    -v "$root:/src:ro" -v "$toolchain:/toolchain:ro" \
    -v "$cache/cargo:/cargo" -v "$cache/build:/build" -v "$root/dist:/out" \
    -e HOME=/tmp -e CARGO_HOME=/cargo -e PATH=/toolchain/bin:/usr/bin:/bin \
    -e SOURCE_DATE_EPOCH="$epoch" -e CARGO_TERM_COLOR=always \
    "$image" sh -euc '
        # A fresh copy of the sources; the build output (target/) is kept between runs.
        find /build -mindepth 1 -maxdepth 1 ! -name target -exec rm -rf {} +
        tar -C /src --exclude=./target --exclude=./dist --exclude=./.git -cf - . | tar -C /build -xf -
        cd /build
        cargo xtask dist --release
        cp dist/vomd-* dist/vomd_* /out/
    '
ls -l "$root"/dist/vomd[-_]*
