@echo off
title Loudstone
cd /d "%~dp0"

if not exist "target\release\loudstone.exe" (
    echo Building Loudstone for the first time - this takes a minute...
    cargo build --release
    if errorlevel 1 (
        echo.
        echo Build failed. Is Rust installed?  https://rustup.rs
        pause
        exit /b 1
    )
)

start "" "target\release\loudstone.exe"
exit /b 0
