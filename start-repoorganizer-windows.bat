@echo off
cd /d "%~dp0"
echo Baue RepoOrganizer (falls Code geaendert wurde)...
cargo build -q -p repoorganizer
if errorlevel 1 (
    echo Fehler beim Kompilieren von RepoOrganizer!
    pause
    exit /b 1
)
start "" "target\debug\repoorganizer.exe" %*
