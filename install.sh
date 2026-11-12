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
fetch tack

if $server; then
    fetch tackd
    units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    mkdir -p "$units"
    cat > "$units/tackd.service" <<'EOF'
[Unit]
Description=tack task list server

[Service]
ExecStart=%h/.local/bin/tackd
Restart=on-failure

[Install]
WantedBy=default.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable tackd
    systemctl --user restart tackd
    echo "tackd is running on port 15470"
    if ! loginctl enable-linger "$(id -un)" 2>/dev/null; then
        echo "to start tackd at boot without logging in, run: sudo loginctl enable-linger $(id -un)"
    fi
fi

case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "warning: $bin is not on PATH, add it in your shell profile" >&2 ;;
esac
