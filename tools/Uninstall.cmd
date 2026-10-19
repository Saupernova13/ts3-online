@echo off
rem Removes ts3-online from the game folder (only our dinput8.dll).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Uninstall
pause
