#!/bin/sh

set -eu

DST="/usr/local/bin"
BASE="/opt/dpl"

if [ "$(id -u)" -ne 0 ]; then
    echo "Error: this script must be run as root" >&2
    exit 1
fi

if ! command -v podman >/dev/null 2>&1; then
    echo "Warning: podman not found, dpl requires podman to run containers" >&2
fi

case "$(uname -m)" in
    x86_64) arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *)
        echo "Error: unsupported architecture $(uname -m)" >&2
        exit 1
        ;;
esac

# DPL_VERSION=1.2.3 pins a release; default is the latest published one.
repo="https://github.com/cesbo/dpl/releases"
if [ -n "${DPL_VERSION:-}" ]; then
    version="${DPL_VERSION#v}"
else
    # /releases/latest redirects to /releases/tag/vX.Y.Z (no API, no rate limit)
    latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$repo/latest")
    version="${latest##*/v}"
fi

case "$version" in
    [0-9]*) ;;
    *)
        echo "Error: cannot determine the release version" >&2
        exit 1
        ;;
esac

# Same version and already registered: nothing to do (no restart either).
if [ -x "$DST/dpl" ] && [ -f /etc/systemd/system/dpl.service ] &&
    [ "$("$DST/dpl" -V 2>/dev/null | awk '{print $NF}')" = "$version" ]; then
    echo "dpl $version is already installed, version not changed"
    exit 0
fi

src_url="$repo/download/v$version/dpl-linux-${arch}.tar.gz"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$src_url" | tar -C "$tmp" -xzf -

new=$("$tmp/dpl" -V)
old=""
if [ -x "$DST/dpl" ]; then
    old=$("$DST/dpl" -V 2>/dev/null || true)
fi

if [ -n "$old" ] && [ "$old" != "$new" ]; then
    echo "Upgrading: $old -> $new"
elif [ -n "$old" ]; then
    echo "Reinstalling: $new"
else
    echo "Version: $new"
fi

# rm first: overwriting a running binary fails with ETXTBSY
rm -f "$DST/dpl"
install -m 755 "$tmp/dpl" "$DST/dpl"

mkdir -p "$BASE/conf" "$BASE/state" "$BASE/secrets" "$BASE/backup"

cat << EOF > /etc/systemd/system/dpl.service
[Unit]
Description=dpl service
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
Environment="DPL_BASE=$BASE"
ExecStart=$DST/dpl serve
ExecReload=/bin/kill -s HUP \$MAINPID
KillMode=process
TimeoutStopSec=300
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable dpl
systemctl restart dpl

echo "🎉 dpl is successfully installed"
