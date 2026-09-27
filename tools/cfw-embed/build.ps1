# Builds cfw-embed.exe with the pinned llvm-mingw toolchain.
#
# The shipped libneedle.a is clang/libc++ (std::__1) built for mingw/UCRT:
#   - MSVC link.exe rejects its COMDAT form (LNK1143) — verified 2026-09-26.
#   - Rust's windows-gnu target links its own msvcrt crt2.o, which collides
#     with UCRT libc++ — verified 2026-09-26.
#   - llvm-mingw clang++ -static links it exactly the way the vendor built
#     needle.exe. Verified: dim=3072, deterministic embeds, ~4.4 ms/embed.
#
# Usage:
#   .\build.ps1 -ToolchainDir <llvm-mingw root> [-NeedleDir <dir with
#     libneedle.a + needle.h>] [-OutDir <dir for cfw-embed.exe>]
#
# Toolchain: https://github.com/mstorsjo/llvm-mingw/releases (ucrt-x86_64).
# NeedleDir defaults to $env:CFW_NEEDLE_DIR (same inputs PR 1 pins).
# CI replicates this script and publishes the exe as a pinned manifest
# download; local builds are for development and testing.

param(
    [Parameter(Mandatory = $true)][string]$ToolchainDir,
    [string]$NeedleDir = $env:CFW_NEEDLE_DIR,
    [string]$OutDir = (Join-Path $PSScriptRoot "dist")
)

$ErrorActionPreference = "Stop"

if (-not $NeedleDir) {
    throw "NeedleDir (or CFW_NEEDLE_DIR) must point at the dir containing libneedle.a and needle.h (PR 1 build inputs)"
}
$lib = Join-Path $NeedleDir "libneedle.a"
$header = Join-Path $NeedleDir "needle.h"
foreach ($f in @($lib, $header)) {
    if (-not (Test-Path $f)) { throw "missing build input: $f" }
}

# Verify the build inputs against the PR 1 pins before linking.
$expected = @{
    "libneedle.a" = "6fb0b9bccfa9f54d46e05a279273c15021570a53a8b3945613d80d299ca1f634"
    "needle.h"    = "3aa713942528d944598458cecb4a262f2cc49349bec63355f91df0b159964e55"
}
foreach ($name in $expected.Keys) {
    $path = Join-Path $NeedleDir $name
    $hash = (Get-FileHash $path -Algorithm SHA256).Hash.ToLower()
    if ($hash -ne $expected[$name]) {
        throw "pin mismatch for ${name}: $hash (expected $($expected[$name]))"
    }
}

$clang = Join-Path $ToolchainDir "bin\x86_64-w64-mingw32-clang++.exe"
if (-not (Test-Path $clang)) { throw "clang++ not found at $clang" }

New-Item -ItemType Directory -Force $OutDir | Out-Null
& $clang -O2 -DNDEBUG `
    "-Wl,/Brepro" `
    -I $NeedleDir -L $NeedleDir `
    (Join-Path $PSScriptRoot "cfw-embed.cpp") `
    -lneedle -static `
    -o (Join-Path $OutDir "cfw-embed.exe")
if ($LASTEXITCODE -ne 0) { throw "cfw-embed build failed" }

$exe = Join-Path $OutDir "cfw-embed.exe"
Write-Host "built: $exe ($((Get-Item $exe).Length) bytes)"
Write-Host "sha256: $((Get-FileHash $exe -Algorithm SHA256).Hash.ToLower())"
