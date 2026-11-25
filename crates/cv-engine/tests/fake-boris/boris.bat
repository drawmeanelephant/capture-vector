@echo off
rem fake-boris (Windows): same contract as the POSIX `boris` shim.
setlocal EnableExtensions DisableDelayedExpansion
set "D=%~dp0"
>>"%D%argv.log" echo(%*
set > "%D%env.log"
if "%~1"=="--version" goto version
if "%~1"=="--build-info" goto buildinfo
if "%~1"=="watch" goto watch
if "%~1"=="nostr" goto nostr
goto verb

:version
type "%D%version.txt"
exit /b 0

:buildinfo
type "%D%build-info.txt"
exit /b 0

:watch
if exist "%D%watch-refuse.txt" goto watchrefuse
if exist "%D%watch.ndjson" type "%D%watch.ndjson" 1>&2
rem A grandchild holding stderr: only a job kill closes the pipe.
if exist "%D%watch-hold" ping -n 300 127.0.0.1 >NUL
set "C=0"
if exist "%D%watch-exit.txt" set /p C=<"%D%watch-exit.txt"
exit /b %C%

:watchrefuse
type "%D%watch-refuse.txt" 1>&2
exit /b 2

:nostr
set "KEY="
set /p KEY=
>"%D%stdin.txt" <nul set /p "=%KEY%"
goto verb

:verb
set "V=%~1"
if "%V%"=="init" if not "%~2"=="--help" if not "%~2"=="" (
  rem Test-double filesystem effect: materialize an installed captured tree.
  if not exist "%~2" mkdir "%~2"
  if exist "%D%verbs\init\tree\" xcopy /E /I /Y /Q "%D%verbs\init\tree" "%~2" >NUL
)
rem IR mode (build --out DIR) replays its own capture when one is loaded.
if "%~2"=="--out" if exist "%D%verbs\%V%-out\" set "V=%V%-out"
if not exist "%D%verbs\%V%\" goto usage
if exist "%D%verbs\%V%\stdout.txt" type "%D%verbs\%V%\stdout.txt"
if exist "%D%verbs\%V%\stderr.txt" type "%D%verbs\%V%\stderr.txt" 1>&2
if exist "%D%verbs\%V%\hold" ping -n 300 127.0.0.1 >NUL
set "C=0"
if exist "%D%verbs\%V%\exit-code.txt" set /p C=<"%D%verbs\%V%\exit-code.txt"
exit /b %C%

:usage
echo usage: fake-boris has no '%V%' 1>&2
exit /b 2
