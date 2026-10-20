@echo off
rem Updates ts3-online to the latest release (see README.md).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Update
pause
