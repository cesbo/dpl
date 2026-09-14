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

echo "Installing..."

case "$(uname -m)" in
    x86_64) arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *)
        echo "Error: unsupported architecture $(uname -m)" >&2
        exit 1
        ;;
esac

src_url="https://cdn.cesbo.com/dpl/latest/dpl-linux-${arch}.tar.gz"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$src_url" | tar -C "$tmp" -xzf -

"$tmp/dpl" -V

# rm first: overwriting a running binary fails with ETXTBSY
rm -f "$DST/dpl"
install -m 755 "$tmp/dpl" "$DST/dpl"

echo "Creating base directory..."

mkdir -p "$BASE/conf" "$BASE/state" "$BASE/secrets" "$BASE/backup"

echo "Registering system service..."

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
echo "Documentation can be found at: https://dpl.cesbo.com"
