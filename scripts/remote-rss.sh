#!/bin/sh
# Memory with many remote images from a server on 127.0.0.1 (never the network):
# `scripts/remote-rss.sh [N [big|tiny|grow]]` (N 0 or none: the mode's own), see examples/remote_rss.rs for what each does.
# The server makes the images as they are asked for; it and everything else is stopped (and the
# temporary directory removed) when done.
set -eu
n=${1:-}
mode=${2:-big}
dir=$(mktemp -d)
trap 'kill "$server" 2>/dev/null || true; rm -rf "$dir"' EXIT INT TERM
cat >"$dir/server.py" <<'PY'
import struct, sys, time, zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

def chunk(t, d):
    return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

def png(w, h):
    head = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    return head + chunk(b"IDAT", zlib.compress(b"\0" * (w * 3 * h + h))) + chunk(b"IEND", b"")

BIG = png(40, 30)
TINY = png(1, 1)

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def log_message(self, *a):
        pass

    def do_GET(self):
        kind = self.path.split("/")[1]
        if kind == "tiny":
            body = TINY
        elif kind == "big":
            # The header is all that is looked at, then 4 MiB.
            body = BIG + b"\0" * (4 << 20)
        elif kind == "stall":
            # Says 7 MiB, sends 6.5 MiB and stops: a transfer that doesn't finish.
            self.send_response(200)
            self.send_header("Content-Length", str(7 << 20))
            self.end_headers()
            try:
                self.wfile.write(BIG + b"\0" * (13 << 19))
                self.wfile.flush()
                time.sleep(30)
            except OSError:
                pass
            return
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except OSError:
            pass

s = ThreadingHTTPServer(("127.0.0.1", 0), H)
print(s.server_address[1], flush=True)
s.serve_forever()
PY
python3 -I -u "$dir/server.py" >"$dir/port" 2>"$dir/log" &
server=$!
port=
for _ in $(seq 50); do
    port=$(head -1 "$dir/port")
    [ -n "$port" ] && break
    sleep 0.1
done
[ -n "$port" ] || { echo "no server" >&2; exit 1; }
cd "$(dirname "$0")/.."
XDG_CONFIG_HOME="$dir/config" \
    cargo run --release --example remote_rss -- "http://127.0.0.1:$port" "${n:-0}" "$mode"
