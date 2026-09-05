@echo off
rem Pair / discover Midea units and write config.json.
rem
rem No venv and no interpreter: this is the server binary talking to your air
rem conditioners directly, which is also why it works before the service does.
setlocal
if "%AC_CONFIG%"=="" set "AC_CONFIG=%ProgramData%\breeze-core\config.json"
echo Pairing Breeze Core units into "%AC_CONFIG%".
echo.
echo The API key is written to that file (it is not printed here - terminal
echo scrollback outlives the moment). Read it from there when the panel or the
echo app asks for it.
echo.
"%~dp0breeze-core.exe" pair --out "%AC_CONFIG%" %*
echo.
echo When it has found your units, start the service:
echo     nssm start BreezeCore      ^(or: sc start BreezeCore^)
echo.
pause
