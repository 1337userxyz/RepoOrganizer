@echo off
setlocal enabledelayedexpansion

REM === Projekt-Konfiguration (pro App anpassen) ===
set "APP_NAME=RepoOrganizer"
set "BIN_NAME=repoorganizer.exe"
set "CARGO_ARGS=-p repoorganizer"
set "NEED_RUST_KIT=1"
set "NEED_SLINT_KIT=1"

set "SCRIPT_DIR=%~dp0"
if "%SCRIPT_DIR:~-1%"=="\" set "SCRIPT_DIR=%SCRIPT_DIR:~0,-1%"
for %%I in ("%SCRIPT_DIR%\..") do set "PARENT_DIR=%%~fI"
if "%PARENT_DIR:~-1%"=="\" set "PARENT_DIR=%PARENT_DIR:~0,-1%"

REM === Standardwerte (falls starter-settings.conf fehlt) ===
set "DEFAULT_PROFILE=release"
set "TARGET_DIRS_WIN={SCRIPT_DIR}\target;C:\cargo-target\{APP}"
set "MIN_FREE_MB_FOR_TARGET=1024"
set "KIT_SEARCH_DIRS_WIN={PARENT};{USERPROFILE}\Projekte;R:\git;C:\git"
set "KIT_MISSING_ACTION=clone"
set "KIT_GITHUB_OWNER=1337userxyz"
set "CARGO_HOME_DIRS_WIN=R:\.cargo;{USERPROFILE}\.cargo"
set "RUSTUP_HOME_DIRS_WIN=R:\.rustup;{USERPROFILE}\.rustup"
set "LINK_CARGO_REGISTRY_TO_USERPROFILE=1"
set "CLEAN_OTHER_PROFILE=auto"
set "STRIP_PDB_WIN=1"
set "CARGO_JOBS=0"
set "RUN_MODE=build_and_run"
set "PAUSE_ON_ERROR=1"

REM === Starter-Settings laden (Global -> Projekt -> Lokal) ===
call :load_settings "%PARENT_DIR%\starter-settings.conf"
call :load_settings "%SCRIPT_DIR%\starter-settings.conf"
call :load_settings "%PARENT_DIR%\starter-settings.local.conf"
call :load_settings "%SCRIPT_DIR%\starter-settings.local.conf"

REM === CLI-Argumente parsen ===
set "DO_CLEAN=0"
set "CLI_PROFILE="
set "APP_ARGS="

:parse_args
if "%~1"=="" goto args_done
if /I "%~1"=="--clean" (
    set "DO_CLEAN=1"
    shift
    goto parse_args
)
if /I "%~1"=="--release" (
    set "CLI_PROFILE=release"
    shift
    goto parse_args
)
if /I "%~1"=="--debug" (
    set "CLI_PROFILE=debug"
    shift
    goto parse_args
)
if /I "%~1"=="--build-only" (
    set "RUN_MODE=build_only"
    shift
    goto parse_args
)
if /I "%~1"=="--run-if-exists" (
    set "RUN_MODE=run_if_exists"
    shift
    goto parse_args
)
set "APP_ARGS=!APP_ARGS! %1"
shift
goto parse_args
:args_done

if defined CLI_PROFILE (
    set "PROFILE=%CLI_PROFILE%"
) else (
    set "PROFILE=%DEFAULT_PROFILE%"
)

REM === 1. Rust Toolchain & Cargo Registry vorbereiten ===
call :setup_cargo_env

REM === 2. Benoetigte Kits sicherstellen (Such-Kaskade + Fallback) ===
if "%NEED_RUST_KIT%"=="1" (
    call :ensure_kit "rust-kit"
    if errorlevel 1 goto build_error
)
if "%NEED_SLINT_KIT%"=="1" (
    call :ensure_kit "slint-desktop-kit"
    if errorlevel 1 goto build_error
)

REM === 3. Build-Zielverzeichnis (CARGO_TARGET_DIR) mit Fallback bestimmen ===
call :resolve_target_dir

if /I "%PROFILE%"=="release" (
    set "TARGET_SUB=release"
    set "OTHER_SUB=debug"
    set "CARGO_PROFILE_FLAG=--release"
) else (
    set "TARGET_SUB=debug"
    set "OTHER_SUB=release"
    set "CARGO_PROFILE_FLAG="
)

set "EXE_PATH=%CARGO_TARGET_DIR%\%TARGET_SUB%\%BIN_NAME%"

REM === Schnellstart falls RUN_MODE=run_if_exists ===
if /I "%RUN_MODE%"=="run_if_exists" if "%DO_CLEAN%"=="0" if exist "%EXE_PATH%" (
    echo [%APP_NAME%] Starte vorhandenes Binary [%PROFILE%]: %EXE_PATH%
    goto launch_app
)

pushd "%SCRIPT_DIR%" >nul

if "%DO_CLEAN%"=="1" (
    echo [%APP_NAME%] Fuehre cargo clean aus...
    cargo clean
)

set "JOBS_FLAG="
if not "%CARGO_JOBS%"=="" if not "%CARGO_JOBS%"=="0" (
    set "JOBS_FLAG=-j %CARGO_JOBS%"
)

echo [%APP_NAME%] Baue Profil '%PROFILE%' [Target: %CARGO_TARGET_DIR%]...
cargo build %CARGO_PROFILE_FLAG% %JOBS_FLAG% %CARGO_ARGS%
if errorlevel 1 (
    popd >nul
    goto build_error
)

popd >nul

REM === 4. Speicherplatz-Schonung (anderes Profil & .pdb bereinigen) ===
call :cleanup_target

if /I "%RUN_MODE%"=="build_only" (
    echo [%APP_NAME%] Build abgeschlossen [RUN_MODE=build_only].
    exit /b 0
)

:launch_app
if not exist "%EXE_PATH%" (
    if exist "%SCRIPT_DIR%\target\%TARGET_SUB%\%BIN_NAME%" (
        set "EXE_PATH=%SCRIPT_DIR%\target\%TARGET_SUB%\%BIN_NAME%"
    )
)
if not exist "%EXE_PATH%" (
    echo [Fehler] Binary nicht gefunden: %EXE_PATH%
    goto build_error
)

start "" "%EXE_PATH%" %APP_ARGS%
exit /b 0

:build_error
echo [Fehler] Build oder Start von %APP_NAME% fehlgeschlagen.
if "%PAUSE_ON_ERROR%"=="1" pause
exit /b 1

REM ============================================================================
REM Hilfsfunktionen
REM ============================================================================

:load_settings
if not exist "%~1" exit /b 0
for /f "usebackq eol=# tokens=1,* delims==" %%A in ("%~1") do (
    set "K=%%A"
    set "V=%%B"
    if defined K (
        for /f "tokens=* delims= " %%X in ("!K!") do set "K=%%X"
        if not "!K:~0,1!"==";" if not "!K!"=="" (
            set "!K!=!V!"
        )
    )
)
exit /b 0

:expand_placeholders
call set "EXP_VAL=%%%~1%%"
if not defined EXP_VAL exit /b 0
set "EXP_VAL=!EXP_VAL:{APP}=%APP_NAME%!"
set "EXP_VAL=!EXP_VAL:{SCRIPT_DIR}=%SCRIPT_DIR%!"
set "EXP_VAL=!EXP_VAL:{PARENT}=%PARENT_DIR%!"
set "EXP_VAL=!EXP_VAL:{USERPROFILE}=%USERPROFILE%!"
set "EXP_VAL=!EXP_VAL:{HOME}=%USERPROFILE%!"
set "%~1=!EXP_VAL!"
exit /b 0

:setup_cargo_env
if not defined RUSTUP_LOG set "RUSTUP_LOG=error"
call :expand_placeholders CARGO_HOME_DIRS_WIN
call :expand_placeholders RUSTUP_HOME_DIRS_WIN

if not defined RUSTUP_HOME (
    for %%D in ("%RUSTUP_HOME_DIRS_WIN:;=" "%") do (
        if not defined RUSTUP_HOME if exist "%%~D\toolchains" set "RUSTUP_HOME=%%~D"
    )
)
if not defined CARGO_HOME (
    for %%D in ("%CARGO_HOME_DIRS_WIN:;=" "%") do (
        if not defined CARGO_HOME if exist "%%~D\bin\cargo.exe" set "CARGO_HOME=%%~D"
    )
)
where cargo >nul 2>&1
if errorlevel 1 (
    if defined CARGO_HOME if exist "%CARGO_HOME%\bin\cargo.exe" (
        set "PATH=%CARGO_HOME%\bin;%PATH%"
    )
    for %%D in ("%CARGO_HOME_DIRS_WIN:;=" "%") do (
        if exist "%%~D\bin\cargo.exe" set "PATH=%%~D\bin;!PATH!"
    )
)
if "%LINK_CARGO_REGISTRY_TO_USERPROFILE%"=="1" if defined CARGO_HOME (
    if /I "!CARGO_HOME:~0,3!"=="R:\" (
        if not exist "%USERPROFILE%\.cargo\registry" mkdir "%USERPROFILE%\.cargo\registry" 2>nul
        fsutil reparsepoint query "!CARGO_HOME!\registry" >nul 2>&1
        if errorlevel 1 (
            if exist "!CARGO_HOME!\registry" rmdir /s /q "!CARGO_HOME!\registry" 2>nul
            mklink /J "!CARGO_HOME!\registry" "%USERPROFILE%\.cargo\registry" >nul 2>&1
        )
    )
)
exit /b 0

:ensure_kit
set "KIT_NAME=%~1"
set "KIT_TARGET=%PARENT_DIR%\%KIT_NAME%"
if exist "%KIT_TARGET%\Cargo.toml" exit /b 0

call :expand_placeholders KIT_SEARCH_DIRS_WIN
for %%D in ("%KIT_SEARCH_DIRS_WIN:;=" "%") do (
    set "CAND_BASE=%%~D"
    if exist "!CAND_BASE!\%KIT_NAME%\Cargo.toml" (
        if /I not "!CAND_BASE!\%KIT_NAME%"=="%KIT_TARGET%" (
            if exist "%KIT_TARGET%" rmdir "%KIT_TARGET%" >nul 2>&1
            echo [Kit] Verlinke %KIT_NAME% aus "!CAND_BASE!\%KIT_NAME%"...
            mklink /J "%KIT_TARGET%" "!CAND_BASE!\%KIT_NAME%" >nul 2>&1
            if exist "%KIT_TARGET%\Cargo.toml" exit /b 0
        )
    )
)

if /I "%KIT_MISSING_ACTION%"=="clone" (
    if exist "%KIT_TARGET%" rmdir "%KIT_TARGET%" >nul 2>&1
    echo [Kit] Klone fehlende Abhaengigkeit %KIT_NAME% von GitHub nach "%KIT_TARGET%"...
    git clone "https://github.com/%KIT_GITHUB_OWNER%/%KIT_NAME%.git" "%KIT_TARGET%"
    if errorlevel 1 exit /b 1
    exit /b 0
)

echo [Fehler] Benoetigtes Kit '%KIT_NAME%' wurde in keinem Suchpfad gefunden (%KIT_SEARCH_DIRS_WIN%).
exit /b 1

:resolve_target_dir
call :expand_placeholders TARGET_DIRS_WIN
set "SELECTED_TARGET="
for %%T in ("%TARGET_DIRS_WIN:;=" "%") do (
    if not defined SELECTED_TARGET (
        set "CAND_TARGET=%%~T"
        for %%D in ("!CAND_TARGET!") do set "CAND_DRV=%%~dD"
        if exist "!CAND_DRV!\" (
            set "SPACE_OK=1"
            if not "%MIN_FREE_MB_FOR_TARGET%"=="" if not "%MIN_FREE_MB_FOR_TARGET%"=="0" (
                set "FREE_MB="
                for /f "usebackq tokens=*" %%F in (`powershell -NoProfile -Command "[math]::Floor([System.IO.DriveInfo]::new('!CAND_DRV!').AvailableFreeSpace / 1MB)" 2^>nul`) do set "FREE_MB=%%F"
                if defined FREE_MB (
                    if !FREE_MB! LSS %MIN_FREE_MB_FOR_TARGET% (
                        echo [%APP_NAME%] Hinweis: Laufwerk !CAND_DRV! hat nur noch !FREE_MB! MB frei [Minimum: %MIN_FREE_MB_FOR_TARGET% MB] - weiche auf naechsten Pfad aus...
                        set "SPACE_OK=0"
                    )
                )
            )
            if "!SPACE_OK!"=="1" set "SELECTED_TARGET=!CAND_TARGET!"
        )
    )
)
if not defined SELECTED_TARGET set "SELECTED_TARGET=%SCRIPT_DIR%\target"
set "CARGO_TARGET_DIR=%SELECTED_TARGET%"
exit /b 0

:cleanup_target
set "DO_CLEAN_OTHER=0"
if "%CLEAN_OTHER_PROFILE%"=="1" set "DO_CLEAN_OTHER=1"
if /I "%CLEAN_OTHER_PROFILE%"=="auto" (
    if /I "%CARGO_TARGET_DIR:~0,3%"=="R:\" set "DO_CLEAN_OTHER=1"
)
if "%DO_CLEAN_OTHER%"=="1" (
    if exist "%CARGO_TARGET_DIR%\%OTHER_SUB%" (
        rmdir /s /q "%CARGO_TARGET_DIR%\%OTHER_SUB%" >nul 2>&1
    )
)
if "%STRIP_PDB_WIN%"=="1" (
    if exist "%CARGO_TARGET_DIR%\%TARGET_SUB%" (
        del /s /q "%CARGO_TARGET_DIR%\%TARGET_SUB%\*.pdb" >nul 2>&1
    )
)
exit /b 0
