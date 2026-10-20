@echo off
rem Starts two copies of Toy Story 3 side by side, the second joined to the first.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0local-coop.ps1"
pause
