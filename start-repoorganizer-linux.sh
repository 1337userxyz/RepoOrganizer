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
SLINT_KIT_DIR="$PARENT_DIR/slint-desktop-kit"

if [ ! -f "$SLINT_KIT_DIR/Cargo.toml" ]; then
    rm -rf "$SLINT_KIT_DIR" 2>/dev/null || true
    if [ -f "$HOME/Projekte/slint-desktop-kit/Cargo.toml" ]; then
        echo "🔗 Verlinke Abhängigkeit slint-desktop-kit aus ~/Projekte..."
        ln -s "$HOME/Projekte/slint-desktop-kit" "$SLINT_KIT_DIR"
    elif [ -f "$HOME/projekte/slint-desktop-kit/Cargo.toml" ]; then
        echo "🔗 Verlinke Abhängigkeit slint-desktop-kit aus ~/projekte..."
        ln -s "$HOME/projekte/slint-desktop-kit" "$SLINT_KIT_DIR"
    elif [ -f "$HOME/git/slint-desktop-kit/Cargo.toml" ]; then
        echo "🔗 Verlinke Abhängigkeit slint-desktop-kit aus ~/git..."
        ln -s "$HOME/git/slint-desktop-kit" "$SLINT_KIT_DIR"
    else
        echo "📥 Klone fehlende Abhängigkeit slint-desktop-kit von GitHub..."
        git clone https://github.com/1337userxyz/slint-desktop-kit.git "$SLINT_KIT_DIR"
    fi
fi

# Falls das Repository veraltet ist und benötigte Crates fehlen (z.B. kit-theme):
if [ -d "$SLINT_KIT_DIR/.git" ] && [ ! -f "$SLINT_KIT_DIR/crates/kit-theme/Cargo.toml" ]; then
    echo "🔄 Aktualisiere slint-desktop-kit (git pull)..."
    git -C "$SLINT_KIT_DIR" pull --ff-only 2>/dev/null || true
fi

# Bei Änderungen stets aktuell bauen (mit sichtbarer Ausgabe)
echo "🔨 Prüfe / Baue RepoOrganizer..."
set +e
RUSTFLAGS="-C link-arg=-Wl,-rpath,$LD_LIBRARY_PATH ${RUSTFLAGS:-}" cargo build -p repoorganizer
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
