param(
    [string]$GloriaSetup = (Join-Path $PSScriptRoot '..\..\GloriaSetup'),
    [switch]$StageOnly
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$stage = Join-Path $root ('target\package\' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $stage | Out-Null

Push-Location $root
try {
    cargo build --locked --release
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }

    Copy-Item 'target\release\mutatio.exe' (Join-Path $stage 'mutatio.exe')
    Copy-Item 'target\release\mutatio-cli.exe' (Join-Path $stage 'mutatio.com')
    Copy-Item 'LICENSE' (Join-Path $stage 'LICENSE.txt')
    Copy-Item 'THIRD_PARTY_LICENSES.md' (Join-Path $stage 'THIRD_PARTY_LICENSES.md')
    Copy-Item 'README.md' (Join-Path $stage 'README.md')

    if ($StageOnly) {
        if ($env:GITHUB_OUTPUT) { "app-dir=$stage" >> $env:GITHUB_OUTPUT }
        Write-Host "Staged Windows x64 payload at $stage"
        return
    }

    $GloriaSetup = (Resolve-Path $GloriaSetup).Path
    $pack = Join-Path $GloriaSetup 'target\release\gloria-pack.exe'
    $stub = Join-Path $GloriaSetup 'target\release\gloria-stub.exe'
    if (!(Test-Path $pack) -or !(Test-Path $stub)) {
        cargo build --release --manifest-path (Join-Path $GloriaSetup 'Cargo.toml') -p gloria-pack -p gloria-stub
        if ($LASTEXITCODE -ne 0) { throw 'GloriaSetup build failed.' }
    }

    & $pack --app $stage --manifest (Join-Path $root 'gloria.toml') --stub $stub --out (Join-Path $root 'dist')
    if ($LASTEXITCODE -ne 0) { throw 'GloriaSetup packaging failed.' }
} finally {
    Pop-Location
}
