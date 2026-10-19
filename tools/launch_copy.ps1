# Start another copy of Toy Story 3 next to a running one (phase H0 of docs/NETCODE.md).
#
#   powershell -File tools\launch_copy.ps1 [-Copy 2] [-CopySave]
#
# -Copy n     copy number 2-4 (informational: the loader picks the copy number itself, first free wins).
#             Its saves live in %LOCALAPPDATA%\ts3online\copy<n>.
# -CopySave   first copy the real savegames into that folder (overwrites what is there).
#
# Game folder: $env:TS3_DIR if set, else found through Steam (HKCU:\Software\Valve\Steam SteamPath and
# steamapps\libraryfolders.vdf). The game is started through explorer.exe so it runs in your desktop session.
param(
    [ValidateRange(2, 4)][int]$Copy = 2,
    [switch]$CopySave
)

$ErrorActionPreference = 'Stop'
$game = 'Toy Story 3 Complete Edition'
$exeName = "$game.exe"

function Find-GameDir {
    if ($env:TS3_DIR) { return $env:TS3_DIR }
    $steam = (Get-ItemProperty -Path 'HKCU:\Software\Valve\Steam' -Name SteamPath -ErrorAction SilentlyContinue).SteamPath
    if (-not $steam) { throw 'cannot find Steam (set TS3_DIR)' }
    $steam = $steam -replace '/', '\'
    $vdf = Join-Path $steam 'steamapps\libraryfolders.vdf'
    if (-not (Test-Path -LiteralPath $vdf)) { throw "missing $vdf (set TS3_DIR)" }
    $libs = @($steam)
    foreach ($line in Get-Content -LiteralPath $vdf) {
        if ($line -match '^\s*"path"\s*"(.*)"') { $libs += ($Matches[1] -replace '\\\\', '\') }
    }
    foreach ($lib in $libs) {
        # [IO.Path]::Combine, not Join-Path: a library on an unplugged drive must not throw
        $dir = [System.IO.Path]::Combine($lib, "steamapps\common\$game")
        if (Test-Path -LiteralPath $dir -PathType Container) { return $dir }
    }
    throw 'game folder not found (set TS3_DIR)'
}

$dir = Find-GameDir
$exe = Join-Path $dir $exeName
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "missing $exe" }

if ($CopySave) {
    $rel = "Disney\$game\savedata"
    $src = Join-Path $env:APPDATA $rel
    $dst = Join-Path $env:LOCALAPPDATA "ts3online\copy$Copy\$rel"
    if (-not (Test-Path -LiteralPath $src -PathType Container)) { throw "no savegames at $src" }
    New-Item -ItemType Directory -Force -Path $dst | Out-Null
    Copy-Item -Path (Join-Path $src '*') -Destination $dst -Recurse -Force
    Write-Output "copied savegames $src -> $dst"
}

Write-Output "starting $exe (copy number is picked by the loader, expected $Copy)"
& explorer.exe "$exe"
