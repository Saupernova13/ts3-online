@echo off
rem Installs ts3-online into the Steam copy of Toy Story 3 (see README.md).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Setup
pause
