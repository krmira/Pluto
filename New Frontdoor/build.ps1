# Builds the Pluto native C frontend (pluto_interface.c + pluto_bridge.c + pluto_platform.c) with MinGW gcc and Raylib.
$ErrorActionPreference = 'Stop'

$frontendDir = $PSScriptRoot
$repoRoot = Split-Path -Parent $frontendDir
$raylibDir = Join-Path $env:LOCALAPPDATA 'pluto-tools\raylib-5.5_win64_mingw-w64'
$ffiDll = Join-Path $repoRoot 'Backdoor\target\release\pluto_frontdoor_ffi.dll'

if (-not (Get-Command gcc -ErrorAction SilentlyContinue)) { throw 'MinGW gcc not found on PATH. Install the WinLibs POSIX/UCRT toolchain.' }
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Rust cargo not found on PATH.' }
if (-not (Test-Path (Join-Path $raylibDir 'include\raylib.h'))) { throw "Raylib not found at '$raylibDir'." }

# 1. Rust backend shared library (pluto_frontdoor_ffi.dll).
cargo build --release -p pluto-frontdoor-ffi
if (-not (Test-Path $ffiDll)) { throw "Backend shared library not found at '$ffiDll'." }

# 2. Native frontend executable.
& gcc -std=c11 -O2 -Wall -Wextra -Wno-unused-parameter `
    -I"$raylibDir\include" -I"$frontendDir" `
    "$frontendDir\pluto_interface.c" "$frontendDir\pluto_bridge.c" "$frontendDir\pluto_platform.c" `
    -L"$raylibDir\lib" -lraylib -lopengl32 -lgdi32 -lwinmm -lcomdlg32 `
    -o "$frontendDir\pluto.exe"

Write-Host "Built $frontendDir\pluto.exe"
