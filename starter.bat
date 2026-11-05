@echo off
cd /d "%~dp0"
if exist "target\release\repoorganizer.exe" (
    start "" "target\release\repoorganizer.exe" %*
    exit /b 0
)
if exist "target\debug\repoorganizer.exe" (
    start "" "target\debug\repoorganizer.exe" %*
    exit /b 0
)
echo Baue RepoOrganizer...
cargo build -p repoorganizer
if errorlevel 1 (
    pause
    exit /b 1
)
start "" "target\debug\repoorganizer.exe" %*
