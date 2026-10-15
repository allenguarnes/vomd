#!/usr/bin/env bash
# Proves that VOMD never touches the network unless asked.
#
# Usage: scripts/netns-check.sh [path/to/vomd]   (default: target/release/vomd)
#
# Opens a document full of remote images in the real binary, inside a network namespace with no
# interfaces (`unshare -rn`) and with a fake `curl` first on PATH that logs every call. With the
# default policy (`ask`) and nobody pressing I, curl must never run. A second run with
# `remote_images = always` is the control: there curl must run, with `-q` first and the URL
# after `--`. Needs a display (Wayland or X11) and unprivileged user namespaces.
set -euo pipefail
bin=$(realpath "${1:-target/release/vomd}")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/ask/vomd" "$tmp/always/vomd"
cat > "$tmp/bin/curl" <<SH
#!/bin/sh
printf '%s\n' "\$*" >> "$tmp/curl.log"
exit 7
SH
chmod +x "$tmp/bin/curl"
printf 'remote_images = always\n' > "$tmp/always/vomd/settings.ini"
{
  echo '# Remote images'
  for i in $(seq 1 20); do
    echo "![badge $i](https://img.shields.io/badge/n-$i-blue.png) ![photo $i](http://example.com/$i.jpg)"
    echo
  done
} > "$tmp/doc.md"

# Run vomd for a few seconds in an empty network namespace with config dir $1.
run() {
  timeout -s TERM 3 unshare -rn env PATH="$tmp/bin:$PATH" XDG_CONFIG_HOME="$tmp/$1" \
    "$bin" "$tmp/doc.md" || true
}

run ask
if [ -s "$tmp/curl.log" ]; then
  echo "FAIL: curl ran without I:" >&2
  cat "$tmp/curl.log" >&2
  exit 1
fi
echo "PASS ask: no curl, no network access"

run always
if ! [ -s "$tmp/curl.log" ]; then
  echo "FAIL (control): curl never ran with remote_images = always" >&2
  exit 1
fi
if grep -qv '^-q .* -- http' "$tmp/curl.log"; then
  echo "FAIL: curl called without -q first or the URL after --:" >&2
  cat "$tmp/curl.log" >&2
  exit 1
fi
echo "PASS always (control): curl ran $(wc -l < "$tmp/curl.log") times, -q first, URL after --"
