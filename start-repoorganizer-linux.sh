#!/usr/bin/env bash
set -euo pipefail

# === Konfiguration (pro App anpassen) ===
APP_NAME="RepoOrganizer"
BIN_NAME="repoorganizer"
DEFAULT_PROFILE="release"
NEED_RUST_KIT=1
NEED_SLINT_KIT=1

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PARENT_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"

# === CLI-Argumente parsen ===
DO_CLEAN=0
PROFILE="$DEFAULT_PROFILE"
APP_ARGS=()

for arg in "$@"; do
    case "$arg" in
        --clean)
            DO_CLEAN=1
            ;;
        --release)
            PROFILE="release"
            ;;
        --debug)
            PROFILE="debug"
            ;;
        *)
            APP_ARGS+=("$arg")
            ;;
    esac
done

# === Symlink-Self-Healing ===
ensure_kit_link() {
    local kit_name="$1"
    local target_link="$PARENT_DIR/$kit_name"
    local backup_path="$HOME/Projekte/$kit_name"
    if [[ ! -f "$target_link/Cargo.toml" && -f "$backup_path/Cargo.toml" ]]; then
        ln -sfn "$backup_path" "$target_link"
    fi
}

if [[ "$NEED_RUST_KIT" -eq 1 ]]; then
    ensure_kit_link "rust-kit"
fi
if [[ "$NEED_SLINT_KIT" -eq 1 ]]; then
    ensure_kit_link "slint-desktop-kit"
fi

# Verhindert, dass Windows-spezifische target-dir-Einträge (z.B. C:/...) unter Linux landen
export CARGO_TARGET_DIR="$SCRIPT_DIR/target"

run_build_and_exec() {
    cd "$SCRIPT_DIR"

    if [[ "$DO_CLEAN" -eq 1 ]]; then
        echo "[Kit] Führe cargo clean aus..."
        cargo clean
    fi

    local target_sub="debug"
    local other_sub="release"
    if [[ "$PROFILE" == "release" ]]; then
        cargo build --release -p repoorganizer
        target_sub="release"
        other_sub="debug"
    else
        cargo build -p repoorganizer
    fi

    # RAM-Disk-Schonung: jeweils anderes Profil im eigenen target/ entfernen
    if [[ -d "$CARGO_TARGET_DIR/$other_sub" ]]; then
        rm -rf "$CARGO_TARGET_DIR/$other_sub"
    fi

    local bin_path="$CARGO_TARGET_DIR/$target_sub/$BIN_NAME"
    if [[ ! -x "$bin_path" ]]; then
        echo "[Fehler] Binary nicht gefunden: $bin_path" >&2
        exit 1
    fi

    exec "$bin_path" "${APP_ARGS[@]}"
}

if [[ -z "${IN_NIX_SHELL:-}" ]] && command -v nix-shell >/dev/null 2>&1 && [[ -f "$SCRIPT_DIR/shell.nix" ]]; then
    export DO_CLEAN PROFILE
    exec nix-shell "$SCRIPT_DIR/shell.nix" --run "$(printf '%q ' "$0" "$@")"
else
    run_build_and_exec
fi
