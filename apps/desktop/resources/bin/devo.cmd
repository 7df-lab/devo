@echo off
:: devo.cmd — Open the installed Devo desktop application on Windows.
::
:: This script is bundled with the Devo installation and can be added
:: to PATH by the "Install CLI Command" action.

setlocal

set "SCRIPT_DIR=%~dp0"
set "APP_EXE=%SCRIPT_DIR%..\Devo.exe"

if exist "%APP_EXE%" (
    start "" "%APP_EXE%" %*
) else (
    echo Error: Could not find Devo.exe at %APP_EXE% 1>&2
    echo Try launching from the Start Menu instead. 1>&2
    exit /b 1
)
