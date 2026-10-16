#!/usr/bin/env bash
# Validate the AUR packages locally, offline and without installing anything:
# `vomd` builds from `git archive HEAD` (commit first), `vomd-bin` from `cargo xtask dist`.
# Each package is built with makepkg, checked with namcap, unpacked into a scratch root and
# its binary run. Install one for real with `makepkg -si` in the printed directory.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
work=$(mktemp -d /tmp/vomd-aur.XXXXXX)
export CARGO_NET_OFFLINE=true
echo "== work dir: $work"

check() { # name
    local dir=$work/$1
    (cd "$dir" && makepkg -f --noconfirm --nodeps)
    namcap "$dir/PKGBUILD"
    local pkg
    pkg=$(ls "$dir"/"$1"-"$ver"-*.pkg.tar.zst)
    namcap "$pkg"
    mkdir -p "$dir/root"
    tar -C "$dir/root" -xf "$pkg"
    for f in usr/bin/vomd usr/share/applications/vomd.desktop \
             usr/share/icons/hicolor/scalable/apps/vomd.svg "usr/share/licenses/$1/LICENSE"; do
        test -e "$dir/root/$f" || { echo "missing $f in $pkg"; exit 1; }
    done
    desktop-file-validate "$dir/root/usr/share/applications/vomd.desktop"
    "$dir/root/usr/bin/vomd" --version
    echo "== $1: OK ($pkg)"
}

mkdir -p "$work/vomd" "$work/vomd-bin"
cp "$root/packaging/aur/vomd/PKGBUILD" "$work/vomd/"
git -C "$root" archive --prefix="vomd-$ver/" -o "$work/vomd/vomd-$ver.tar.gz" HEAD
check vomd

cp "$root/packaging/aur/vomd-bin/PKGBUILD" "$work/vomd-bin/"
(cd "$root" && cargo xtask dist)
cp "$root/dist/vomd-$ver-x86_64-linux.tar.gz" "$work/vomd-bin/"
check vomd-bin
