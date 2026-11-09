@echo off
setlocal

cd /d "%~dp0"

:: Unterdruecke rustup-Warnungen zu nicht-kanonischen Pfaden auf virtuellen/ImDisk-Laufwerken (z.B. R:\)
if not defined RUSTUP_LOG set "RUSTUP_LOG=error"

:: Cargo / Rust Pfad sicherstellen (unterstuetzt R:\.cargo\bin sowie Userprofile)
where cargo >nul 2>&1
if errorlevel 1 (
    if defined CARGO_HOME (
        if exist "%CARGO_HOME%\bin\cargo.exe" set "PATH=%CARGO_HOME%\bin;%PATH%"
    )
    if exist "R:\.cargo\bin\cargo.exe" (
        set "PATH=R:\.cargo\bin;%PATH%"
    ) else if exist "%USERPROFILE%\.cargo\bin\cargo.exe" (
        set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
    )
)

:: ImDisk/RAM-Disk Fix fuer Cargo Registry:
:: Cargo (tar-Crate) ruft beim Entpacken std::fs::canonicalize() auf. Auf ImDisk-Laufwerken (R:\)
:: schlaegt dies mit "Unzulaessige Funktion (os error 1)" fehl. Eine NTFS-Junction nach
:: %USERPROFILE%\.cargo\registry behebt den Fehler und haelt den Crate-Cache ueber Neustarts persistent.
if exist "R:\.cargo" (
    if not exist "%USERPROFILE%\.cargo\registry" (
        mkdir "%USERPROFILE%\.cargo\registry" 2>nul
    )
    fsutil reparsepoint query "R:\.cargo\registry" >nul 2>&1
    if errorlevel 1 (
        if exist "R:\.cargo\registry" (
            rmdir /s /q "R:\.cargo\registry" 2>nul
        )
        mklink /J "R:\.cargo\registry" "%USERPROFILE%\.cargo\registry" >nul 2>&1
    )
)

:: Abhaengigkeit slint-desktop-kit absichern (insbesondere auf RAM-Disk R:\git)
if not exist "..\slint-desktop-kit\Cargo.toml" (
    if exist "%USERPROFILE%\Projekte\slint-desktop-kit\Cargo.toml" (
        echo Verlinke Abhaengigkeit slint-desktop-kit aus %USERPROFILE%\Projekte...
        mklink /J "..\slint-desktop-kit" "%USERPROFILE%\Projekte\slint-desktop-kit" >nul
    ) else if exist "%USERPROFILE%\git\slint-desktop-kit\Cargo.toml" (
        echo Verlinke Abhaengigkeit slint-desktop-kit aus %USERPROFILE%\git...
        mklink /J "..\slint-desktop-kit" "%USERPROFILE%\git\slint-desktop-kit" >nul
    ) else (
        echo Klone fehlende Abhaengigkeit slint-desktop-kit nach ..\slint-desktop-kit...
        git clone https://github.com/1337userxyz/slint-desktop-kit.git "..\slint-desktop-kit"
        if errorlevel 1 (
            echo [FEHLER] Konnte slint-desktop-kit nicht klonen!
            pause
            exit /b 1
        )
    )
)

echo Pruefe / Baue RepoOrganizer...
cargo build -p repoorganizer
if errorlevel 1 (
    echo.
    echo [FEHLER] Fehler beim Kompilieren von RepoOrganizer!
    echo Falls der Fehler "linker link.exe not found" lautet, starte bitte:
    echo   R:\git\pwsh7-scripts\Setup\DevWorkstation\Apps\VisualStudioBuildTools\Install-VisualStudioBuildTools.bat
    pause
    exit /b 1
)

echo Starte RepoOrganizer GUI...
start "" "target\debug\repoorganizer.exe" %*
