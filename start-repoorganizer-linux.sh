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
    exec nix-shell "$SCRIPT_DIR/shell.nix" --run "\"$SCRIPT_DIR/$SCRIPT_NAME\" $*"
fi

# Für NixOS D-Bus / xdg-desktop-portal Bibliothekspfad bereitstellen
for d in /nix/store/*-dbus-*-lib/lib; do
    if [ -d "$d" ]; then
        export LD_LIBRARY_PATH="$d:${LD_LIBRARY_PATH:-}"
        break
    fi
done

# Bei Änderungen stets aktuell bauen
cargo build -q -p repoorganizer
if [ $? -ne 0 ]; then
    echo "Fehler beim Bauen von RepoOrganizer." >&2
    exit 1
fi

exec "target/debug/repoorganizer" "$@"
