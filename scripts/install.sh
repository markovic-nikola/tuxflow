#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

PREFIX="/usr/local"
USER_INSTALL=false
UNINSTALL=false

usage() {
    echo "Usage: $0 [--user] [--prefix PREFIX] [--uninstall]"
    echo ""
    echo "  --user        Install to ~/.local (no sudo required)"
    echo "  --prefix DIR  Install to custom prefix (default: /usr/local)"
    echo "  --uninstall   Remove installed files"
    echo ""
    echo "No toolkit runtime is needed: the binary links libc alone and loads the"
    echo "desktop's own X11/Wayland, xkbcommon and OpenGL libraries at run time."
    echo "Notification sounds need paplay (optional):"
    echo "  Ubuntu/Debian/Fedora: pulseaudio-utils    Arch: libpulse"
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --user)
            PREFIX="$HOME/.local"
            USER_INSTALL=true
            shift
            ;;
        --prefix)
            PREFIX="$2"
            shift 2
            ;;
        --uninstall)
            UNINSTALL=true
            shift
            ;;
        --help|-h)
            usage
            ;;
        *)
            echo "Unknown option: $1"
            usage
            ;;
    esac
done

FILES=(
    "$PREFIX/bin/tuxflow"
    "$PREFIX/bin/tuxflow-mcp"
    "$PREFIX/share/applications/com.tuxflow.TuxFlow.desktop"
    "$PREFIX/share/metainfo/com.tuxflow.TuxFlow.metainfo.xml"
    "$PREFIX/share/icons/hicolor/scalable/apps/com.tuxflow.TuxFlow.svg"
)

if [ "$UNINSTALL" = true ]; then
    if [ "$USER_INSTALL" = false ] && [ "$(id -u)" -ne 0 ]; then
        echo "Uninstalling from $PREFIX requires root. Re-running with sudo..."
        exec sudo "$0" "$@"
    fi
    echo "Uninstalling TuxFlow from $PREFIX..."
    for f in "${FILES[@]}"; do
        if [ -f "$f" ]; then
            rm -v "$f"
        fi
    done
    echo "Done. You may also want to remove ~/.config/tuxflow/"
    exit 0
fi

if [ "$USER_INSTALL" = false ] && [ "$(id -u)" -ne 0 ]; then
    echo "Installing to $PREFIX requires root. Re-running with sudo..."
    exec sudo "$0" "$@"
fi

echo "Installing TuxFlow to $PREFIX..."

install -Dm755 "$SCRIPT_DIR/tuxflow" "$PREFIX/bin/tuxflow"
install -Dm755 "$SCRIPT_DIR/tuxflow-mcp" "$PREFIX/bin/tuxflow-mcp"
install -Dm644 "$SCRIPT_DIR/com.tuxflow.TuxFlow.desktop" "$PREFIX/share/applications/com.tuxflow.TuxFlow.desktop"
install -Dm644 "$SCRIPT_DIR/com.tuxflow.TuxFlow.metainfo.xml" "$PREFIX/share/metainfo/com.tuxflow.TuxFlow.metainfo.xml"
install -Dm644 "$SCRIPT_DIR/com.tuxflow.TuxFlow.svg" "$PREFIX/share/icons/hicolor/scalable/apps/com.tuxflow.TuxFlow.svg"

if command -v gtk-update-icon-cache &>/dev/null; then
    gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
fi

if command -v update-desktop-database &>/dev/null; then
    update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true
fi

echo ""
echo "TuxFlow installed successfully!"
echo ""
echo "Notification sounds need paplay (optional):"
echo "  Ubuntu/Debian/Fedora: pulseaudio-utils    Arch: libpulse"
if [ "$USER_INSTALL" = true ]; then
    echo ""
    echo "Make sure ~/.local/bin is in your PATH."
fi
