@echo off
rem The program to name in AGENTHANGOUT_TASK_COMMAND on Windows. The doorbell runs one program,
rem with nothing split off it, so this small file starts pwsh on the worker. stdin and stdout pass
rem straight through.
pwsh -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%~dp0task-worker.ps1"
exit /b %ERRORLEVEL%
