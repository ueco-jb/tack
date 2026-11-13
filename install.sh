#!/bin/sh
set -eu

repo=ueco-jb/tack
bin="$HOME/.local/bin"
server=false

for arg in "$@"; do
    case $arg in
        --server) server=true ;;
        *) echo "install.sh: unknown option $arg" >&2; exit 1 ;;
    esac
done

case $(uname -m) in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    armv6l | armv7l) arch=armv6 ;;
    *) echo "install.sh: no release for $(uname -m)" >&2; exit 1 ;;
esac

if $server && ! command -v systemctl >/dev/null 2>&1; then
    echo "install.sh: --server needs systemd" >&2
    exit 1
fi

fetch() {
    curl -fsSL "https://github.com/$repo/releases/latest/download/$1-$arch-linux" -o "$bin/$1.new"
    chmod +x "$bin/$1.new"
    mv "$bin/$1.new" "$bin/$1"
    echo "installed $bin/$1"
}

mkdir -p "$bin"
if $server; then
    fetch tackd
    sudo tee /etc/systemd/system/tackd.service >/dev/null <<EOF
[Unit]
Description=tack task list server
After=network.target

[Service]
User=$(id -un)
ExecStart=$bin/tackd
Restart=on-failure

[Install]
WantedBy=multi-user.target
EOF
    sudo systemctl daemon-reload
    sudo systemctl enable tackd
    sudo systemctl restart tackd
    echo "tackd is running on port 15470"
else
    fetch tack
fi

case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "warning: $bin is not on PATH, add it in your shell profile" >&2 ;;
esac
