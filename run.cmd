@echo off
set "PATH=%USERPROFILE%\.cargo\bin;C:\Program Files\nodejs;%PATH%"
call .\node_modules\.bin\tauri.cmd %*
