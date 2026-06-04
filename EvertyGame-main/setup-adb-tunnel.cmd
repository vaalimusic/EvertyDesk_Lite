@echo off
setlocal

set PORT=%~1
if "%PORT%"=="" set PORT=5001

set ADB_EXE=%LOCALAPPDATA%\Android\Sdk\platform-tools\adb.exe
if exist "%ADB_EXE%" goto run_adb

set ADB_EXE=adb.exe

:run_adb
"%ADB_EXE%" start-server
if errorlevel 1 goto adb_failed

"%ADB_EXE%" reverse tcp:%PORT% tcp:%PORT%
if errorlevel 1 goto adb_failed

echo.
echo ADB tunnel is ready on tcp:%PORT%
"%ADB_EXE%" reverse --list
goto done

:adb_failed
echo.
echo Failed to configure ADB reverse on port %PORT%.
echo Make sure USB debugging is enabled and the phone is authorized in adb.
exit /b 1

:done
endlocal
