@echo off
setlocal enabledelayedexpansion

REM === Konfiguration (pro App anpassen) ===
set "APP_NAME=RepoOrganizer"
set "BIN_NAME=repoorganizer.exe"
set "DEFAULT_PROFILE=release"
set "NEED_RUST_KIT=1"
set "NEED_SLINT_KIT=1"

set "SCRIPT_DIR=%~dp0"
if "%SCRIPT_DIR:~-1%"=="\" set "SCRIPT_DIR=%SCRIPT_DIR:~0,-1%"
for %%I in ("%SCRIPT_DIR%\..") do set "PARENT_DIR=%%~fI"

REM === CLI-Argumente parsen ===
set "DO_CLEAN=0"
set "PROFILE=%DEFAULT_PROFILE%"
set "APP_ARGS="

:parse_args
if "%~1"=="" goto args_done
if /I "%~1"=="--clean" (
    set "DO_CLEAN=1"
    shift
    goto parse_args
)
if /I "%~1"=="--release" (
    set "PROFILE=release"
    shift
    goto parse_args
)
if /I "%~1"=="--debug" (
    set "PROFILE=debug"
    shift
    goto parse_args
)
set "APP_ARGS=!APP_ARGS! %1"
shift
goto parse_args
:args_done

REM === Symlink/Junction-Self-Healing ===
if "%NEED_RUST_KIT%"=="1" (
    if not exist "%PARENT_DIR%\rust-kit\Cargo.toml" (
        if exist "C:\Users\%USERNAME%\Projekte\rust-kit\Cargo.toml" (
            mklink /J "%PARENT_DIR%\rust-kit" "C:\Users\%USERNAME%\Projekte\rust-kit" >nul 2>&1
        ) else if exist "R:\git\rust-kit\Cargo.toml" (
            mklink /J "%PARENT_DIR%\rust-kit" "R:\git\rust-kit" >nul 2>&1
        )
    )
)
if "%NEED_SLINT_KIT%"=="1" (
    if not exist "%PARENT_DIR%\slint-desktop-kit\Cargo.toml" (
        if exist "C:\Users\%USERNAME%\Projekte\slint-desktop-kit\Cargo.toml" (
            mklink /J "%PARENT_DIR%\slint-desktop-kit" "C:\Users\%USERNAME%\Projekte\slint-desktop-kit" >nul 2>&1
        ) else if exist "R:\git\slint-desktop-kit\Cargo.toml" (
            mklink /J "%PARENT_DIR%\slint-desktop-kit" "R:\git\slint-desktop-kit" >nul 2>&1
        )
    )
)

pushd "%SCRIPT_DIR%" >nul

if "%DO_CLEAN%"=="1" (
    echo [Kit] Fuehre cargo clean aus...
    cargo clean
)

if /I "%PROFILE%"=="release" (
    cargo build --release -p repoorganizer
    if errorlevel 1 goto build_error
    set "TARGET_SUB=release"
    set "OTHER_SUB=debug"
) else (
    cargo build -p repoorganizer
    if errorlevel 1 goto build_error
    set "TARGET_SUB=debug"
    set "OTHER_SUB=release"
)

REM === RAM-Disk-Schonung (nur eigenes target/) ===
if exist "%SCRIPT_DIR%\target\%OTHER_SUB%" (
    rmdir /s /q "%SCRIPT_DIR%\target\%OTHER_SUB%" >nul 2>&1
)
del /s /q "%SCRIPT_DIR%\target\%TARGET_SUB%\*.pdb" >nul 2>&1

set "EXE_PATH=%SCRIPT_DIR%\target\%TARGET_SUB%\%BIN_NAME%"
if not exist "%EXE_PATH%" (
    if defined CARGO_TARGET_DIR (
        set "EXE_PATH=%CARGO_TARGET_DIR%\%TARGET_SUB%\%BIN_NAME%"
    )
)

if not exist "%EXE_PATH%" (
    echo [Fehler] Binary nicht gefunden: %EXE_PATH%
    popd >nul
    exit /b 1
)

start "" "%EXE_PATH%" %APP_ARGS%
popd >nul
exit /b 0

:build_error
echo [Fehler] Build fehlgeschlagen.
popd >nul
exit /b 1
