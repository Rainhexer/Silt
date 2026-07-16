#!/bin/sh
# Silt installer — downloads the latest static binary from GitHub releases.
# Usage: curl -fsSL https://raw.githubusercontent.com/FlyvendeMus/Silt/master/install.sh | sh
set -eu

REPO="FlyvendeMus/Silt"
BIN="silt"

case "$(uname -m)" in
    x86_64 | amd64)   TARGET="x86_64-unknown-linux-musl" ;;
    aarch64 | arm64)  TARGET="aarch64-unknown-linux-musl" ;;
    *)
        echo "error: unsupported architecture: $(uname -m)" >&2
        echo "build from source instead: cargo install --git https://github.com/$REPO" >&2
        exit 1
        ;;
esac

if [ "$(uname -s)" != "Linux" ]; then
    echo "error: silt only supports Linux" >&2
    exit 1
fi

URL="https://github.com/$REPO/releases/latest/download/$BIN-$TARGET.tar.gz"

# Prefer /usr/local/bin when writable, otherwise ~/.local/bin.
if [ -w /usr/local/bin ]; then
    INSTALL_DIR="/usr/local/bin"
else
    INSTALL_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
    mkdir -p "$INSTALL_DIR"
fi

TMPDIR_DL="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_DL"' EXIT

echo "downloading $URL"
if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$URL" -o "$TMPDIR_DL/$BIN.tar.gz"
elif command -v wget >/dev/null 2>&1; then
    wget -q "$URL" -O "$TMPDIR_DL/$BIN.tar.gz"
else
    echo "error: need curl or wget" >&2
    exit 1
fi

tar -xzf "$TMPDIR_DL/$BIN.tar.gz" -C "$TMPDIR_DL"
install -m 755 "$TMPDIR_DL/$BIN" "$INSTALL_DIR/$BIN"

echo "installed $BIN to $INSTALL_DIR/$BIN"

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        echo
        echo "note: $INSTALL_DIR is not in your PATH. Add this to your shell profile:"
        echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
        ;;
esac

"$INSTALL_DIR/$BIN" --version
