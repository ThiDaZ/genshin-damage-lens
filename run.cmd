@echo off
set "PATH=%USERPROFILE%\.cargo\bin;C:\Program Files\nodejs;%PATH%"
taskkill /f /im genshin-damage-lens.exe >nul 2>&1
call .\node_modules\.bin\tauri.cmd %*
