#!/usr/bin/env bash
set -euo pipefail

# === Projekt-Konfiguration (pro App anpassen) ===
APP_NAME="RepoOrganizer"
BIN_NAME="repoorganizer"
CARGO_ARGS=("-p" "repoorganizer")
NEED_RUST_KIT=1
NEED_SLINT_KIT=1

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PARENT_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"

# === Standardwerte (falls starter-settings.conf fehlt) ===
DEFAULT_PROFILE="release"
TARGET_DIRS_LINUX="{SCRIPT_DIR}/target;{HOME}/.cache/cargo-target/{APP}"
MIN_FREE_MB_FOR_TARGET=1024
KIT_SEARCH_DIRS_LINUX="{PARENT};{HOME}/Projekte;/r/git;{HOME}/git"
KIT_MISSING_ACTION="clone"
KIT_GITHUB_OWNER="1337userxyz"
CLEAN_OTHER_PROFILE="auto"
CARGO_JOBS=0
RUN_MODE="build_and_run"
PAUSE_ON_ERROR=1

# === Starter-Settings laden (Global -> Projekt -> Lokal) ===
load_settings() {
    local cfg_file="$1"
    [[ -f "$cfg_file" ]] || return 0
    while IFS='=' read -r raw_key raw_val || [[ -n "$raw_key" ]]; do
        local key
        key="$(echo "$raw_key" | tr -d '\r' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
        [[ -z "$key" || "${key:0:1}" == "#" || "${key:0:1}" == ";" ]] && continue
        local val
        val="$(echo "${raw_val:-}" | tr -d '\r')"
        printf -v "$key" '%s' "$val"
    done < "$cfg_file"
}

expand_placeholders() {
    local s="$1"
    s="${s//\{APP\}/$APP_NAME}"
    s="${s//\{SCRIPT_DIR\}/$SCRIPT_DIR}"
    s="${s//\{PARENT\}/$PARENT_DIR}"
    s="${s//\{HOME\}/$HOME}"
    s="${s//\{USERPROFILE\}/$HOME}"
    printf '%s' "$s"
}

load_settings "$PARENT_DIR/starter-settings.conf"
load_settings "$SCRIPT_DIR/starter-settings.conf"
load_settings "$PARENT_DIR/starter-settings.local.conf"
load_settings "$SCRIPT_DIR/starter-settings.local.conf"

# === CLI-Argumente parsen ===
DO_CLEAN=0
CLI_PROFILE=""
APP_ARGS=()

for arg in "$@"; do
    case "$arg" in
        --clean)
            DO_CLEAN=1
            ;;
        --release)
            CLI_PROFILE="release"
            ;;
        --debug)
            CLI_PROFILE="debug"
            ;;
        --build-only)
            RUN_MODE="build_only"
            ;;
        --run-if-exists)
            RUN_MODE="run_if_exists"
            ;;
        *)
            APP_ARGS+=("$arg")
            ;;
    esac
done

PROFILE="${CLI_PROFILE:-$DEFAULT_PROFILE}"

# === 1. Benoetigte Kits sicherstellen (Such-Kaskade + Fallback) ===
ensure_kit_link() {
    local kit_name="$1"
    local target_link="$PARENT_DIR/$kit_name"
    [[ -f "$target_link/Cargo.toml" ]] && return 0

    local search_dirs
    search_dirs="$(expand_placeholders "$KIT_SEARCH_DIRS_LINUX")"
    IFS=';' read -ra CANDS <<< "$search_dirs"
    for cand_base in "${CANDS[@]}"; do
        [[ -z "$cand_base" ]] && continue
        if [[ -f "$cand_base/$kit_name/Cargo.toml" ]]; then
            if [[ "$cand_base/$kit_name" != "$target_link" ]]; then
                rm -rf "$target_link" 2>/dev/null || true
                echo "[Kit] Verlinke $kit_name aus $cand_base/$kit_name..."
                ln -sfn "$cand_base/$kit_name" "$target_link"
                [[ -f "$target_link/Cargo.toml" ]] && return 0
            fi
        fi
    done

    if [[ "${KIT_MISSING_ACTION,,}" == "clone" ]]; then
        rm -rf "$target_link" 2>/dev/null || true
        echo "[Kit] Klone fehlende Abhängigkeit $kit_name nach $target_link..."
        git clone "https://github.com/$KIT_GITHUB_OWNER/$kit_name.git" "$target_link"
        return 0
    fi

    echo "[Fehler] Benötigtes Kit '$kit_name' wurde in keinem Suchpfad gefunden ($search_dirs)." >&2
    return 1
}

if [[ "$NEED_RUST_KIT" -eq 1 ]]; then
    ensure_kit_link "rust-kit"
fi
if [[ "$NEED_SLINT_KIT" -eq 1 ]]; then
    ensure_kit_link "slint-desktop-kit"
fi

# === 2. Build-Zielverzeichnis (CARGO_TARGET_DIR) mit Fallback bestimmen ===
resolve_target_dir() {
    local target_dirs
    target_dirs="$(expand_placeholders "$TARGET_DIRS_LINUX")"
    IFS=';' read -ra CANDS <<< "$target_dirs"
    for cand in "${CANDS[@]}"; do
        [[ -z "$cand" ]] && continue
        if [[ "$cand" == /r/* && ! -d "/r" ]]; then
            continue
        fi
        local probe="$cand"
        while [[ ! -d "$probe" && "$probe" != "/" ]]; do
            probe="$(dirname "$probe")"
        done
        if [[ -d "$probe" ]]; then
            if [[ "${MIN_FREE_MB_FOR_TARGET:-0}" -gt 0 ]] && command -v df >/dev/null 2>&1; then
                local free_mb
                free_mb="$(df -Pk "$probe" 2>/dev/null | awk 'NR==2 {print int($4/1024)}' || echo "")"
                if [[ -n "$free_mb" && "$free_mb" -lt "$MIN_FREE_MB_FOR_TARGET" ]]; then
                    echo "[$APP_NAME] Hinweis: $probe hat nur noch ${free_mb} MB frei (< ${MIN_FREE_MB_FOR_TARGET} MB) - weiche auf nächsten Pfad aus..." >&2
                    continue
                fi
            fi
            export CARGO_TARGET_DIR="$cand"
            return 0
        fi
    done
    export CARGO_TARGET_DIR="$SCRIPT_DIR/target"
}

resolve_target_dir

# Fallback fuer NVIDIA Mismatch nach Treiber-Update (NixOS)
if command -v nvidia-smi >/dev/null 2>&1; then
    if nvidia-smi 2>&1 | grep -q -i "mismatch"; then
        export LIBGL_ALWAYS_SOFTWARE=1
        export WGPU_BACKEND=gl
    fi
fi

run_build_and_exec() {
    cd "$SCRIPT_DIR"

    local target_sub="debug"
    local other_sub="release"
    local profile_args=()
    if [[ "${PROFILE,,}" == "release" ]]; then
        target_sub="release"
        other_sub="debug"
        profile_args=("--release")
    fi

    local bin_path="$CARGO_TARGET_DIR/$target_sub/$BIN_NAME"

    if [[ "${RUN_MODE,,}" == "run_if_exists" && "$DO_CLEAN" -eq 0 && -x "$bin_path" ]]; then
        echo "[$APP_NAME] Starte vorhandenes Binary ($PROFILE): $bin_path"
        exec "$bin_path" "${APP_ARGS[@]}"
    fi

    if [[ "$DO_CLEAN" -eq 1 ]]; then
        echo "[$APP_NAME] Führe cargo clean aus..."
        cargo clean
    fi

    local job_args=()
    if [[ "${CARGO_JOBS:-0}" -gt 0 ]]; then
        job_args=("-j" "$CARGO_JOBS")
    fi

    echo "[$APP_NAME] Baue Profil '$PROFILE' (Target: $CARGO_TARGET_DIR)..."
    if ! cargo build "${profile_args[@]}" "${job_args[@]}" "${CARGO_ARGS[@]}"; then
        echo "[Fehler] Build von $APP_NAME fehlgeschlagen." >&2
        if [[ "${PAUSE_ON_ERROR:-1}" -eq 1 && -t 0 ]]; then
            read -r -p "Drücke [Enter] zum Beenden..."
        fi
        exit 1
    fi

    # Speicherplatz-Schonung: anderes Profil entfernen
    local do_clean_other=0
    if [[ "$CLEAN_OTHER_PROFILE" == "1" ]]; then
        do_clean_other=1
    elif [[ "${CLEAN_OTHER_PROFILE,,}" == "auto" && "$CARGO_TARGET_DIR" == /r/* ]]; then
        do_clean_other=1
    fi
    if [[ "$do_clean_other" -eq 1 && -d "$CARGO_TARGET_DIR/$other_sub" ]]; then
        rm -rf "$CARGO_TARGET_DIR/$other_sub"
    fi

    if [[ "${RUN_MODE,,}" == "build_only" ]]; then
        echo "[$APP_NAME] Build abgeschlossen (RUN_MODE=build_only)."
        exit 0
    fi

    if [[ ! -x "$bin_path" ]]; then
        echo "[Fehler] Binary nicht gefunden: $bin_path" >&2
        exit 1
    fi

    exec "$bin_path" "${APP_ARGS[@]}"
}

if [[ -z "${IN_NIX_SHELL:-}" ]] && command -v nix-shell >/dev/null 2>&1 && [[ -f "$SCRIPT_DIR/shell.nix" ]]; then
    export DO_CLEAN PROFILE RUN_MODE
    exec nix-shell "$SCRIPT_DIR/shell.nix" --run "$(printf '%q ' "$0" "$@")"
else
    run_build_and_exec
fi
