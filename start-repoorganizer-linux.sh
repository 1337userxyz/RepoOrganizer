#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT_NAME="$(basename "$0")"
cd "$SCRIPT_DIR"

export PATH="/run/wrappers/bin:/run/current-system/sw/bin:$HOME/.nix-profile/bin:$HOME/.cargo/bin:$PATH"

# Fallback für NVIDIA Mismatch nach Treiber-Update (NixOS)
if command -v nvidia-smi >/dev/null 2>&1; then
    if nvidia-smi 2>&1 | grep -q -i "mismatch"; then
        export LIBGL_ALWAYS_SOFTWARE=1
        export WGPU_BACKEND=gl
    fi
fi

# Wenn NixOS / nix-shell vorhanden ist und wir noch nicht in nix-shell sind:
if [ -z "$IN_NIX_SHELL" ] && command -v nix-shell >/dev/null 2>&1 && [ -f "$SCRIPT_DIR/shell.nix" ]; then
    echo "📦 Initialisiere NixOS Build-Umgebung (nix-shell)..."
    exec nix-shell "$SCRIPT_DIR/shell.nix" --run "\"$SCRIPT_DIR/$SCRIPT_NAME\" $*"
fi

# Für NixOS D-Bus / xdg-desktop-portal Bibliothekspfad bereitstellen
for d in /nix/store/*-dbus-*-lib/lib; do
    if [ -d "$d" ]; then
        export LD_LIBRARY_PATH="$d:${LD_LIBRARY_PATH:-}"
        break
    fi
done

# Abhängigkeit slint-desktop-kit absichern (insbesondere auf RAM-Disk /r)
PARENT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
if [ ! -e "$PARENT_DIR/slint-desktop-kit" ]; then
    rm -f "$PARENT_DIR/slint-desktop-kit" 2>/dev/null || true
    if [ -d "$HOME/Projekte/slint-desktop-kit" ]; then
        echo "🔗 Verlinke Abhängigkeit slint-desktop-kit aus ~/Projekte..."
        ln -s "$HOME/Projekte/slint-desktop-kit" "$PARENT_DIR/slint-desktop-kit"
    elif [ -d "$HOME/projekte/slint-desktop-kit" ]; then
        echo "🔗 Verlinke Abhängigkeit slint-desktop-kit aus ~/projekte..."
        ln -s "$HOME/projekte/slint-desktop-kit" "$PARENT_DIR/slint-desktop-kit"
    fi
fi

# Bei Änderungen stets aktuell bauen (mit sichtbarer Ausgabe)
echo "🔨 Prüfe / Baue RepoOrganizer..."
set +e
cargo build -p repoorganizer
BUILD_STATUS=$?
set -e

if [ $BUILD_STATUS -ne 0 ]; then
    echo ""
    echo "❌ Fehler beim Bauen von RepoOrganizer (Exit Code $BUILD_STATUS)." >&2
    if [ -t 1 ]; then
        read -r -p "Drücke [Enter] zum Beenden..."
    fi
    exit $BUILD_STATUS
fi

echo "🚀 Starte RepoOrganizer GUI..."
exec "target/debug/repoorganizer" "$@"
