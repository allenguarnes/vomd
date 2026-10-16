#!/bin/sh
# Peak RSS with 16 remote images of ~4 MiB each from a server on 127.0.0.1 (never the network):
# `scripts/remote-rss.sh [N]`. Writes the images to a temporary directory and removes it, and
# stops the server, when done.
set -eu
n=${1:-16}
dir=$(mktemp -d)
trap 'kill "$server" 2>/dev/null || true; rm -rf "$dir"' EXIT INT TERM
mkdir "$dir/big"
# A 1 x 1 PNG, then 4 MiB of zeros: the header is all that is looked at.
python3 -I - "$dir/big" "$n" <<'PY'
import struct, sys, zlib
def chunk(t, d):
    return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 40, 30, 8, 2, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(b"\0" * (40 * 3 * 30 + 30))) + chunk(b"IEND", b"")
for i in range(int(sys.argv[2])):
    open(f"{sys.argv[1]}/{i}.png", "wb").write(png + b"\0" * (4 << 20))
PY
python3 -I -u -m http.server 0 --bind 127.0.0.1 --directory "$dir" >"$dir/log" 2>&1 &
server=$!
port=
for _ in $(seq 50); do
    port=$(sed -n 's/.*port \([0-9]*\).*/\1/p' "$dir/log" | head -1)
    [ -n "$port" ] && break
    sleep 0.1
done
[ -n "$port" ] || { echo "no server" >&2; exit 1; }
cd "$(dirname "$0")/.."
XDG_CONFIG_HOME="$dir/config" \
    cargo run --release --example remote_rss -- "http://127.0.0.1:$port" "$n"
