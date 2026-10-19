<#
.SYNOPSIS
  Installs or removes the ts3-online loader (dinput8.dll) for the Steam version of
  Toy Story 3 Complete Edition, and writes a starter ts3online.ini for online play.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File install.ps1 -Name Alice
  powershell -ExecutionPolicy Bypass -File install.ps1 -Firewall
  powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall

.NOTES
  The DLL is taken from next to this script (release zip), else from loader\target\release.
  Set -GameDir (or TS3_DIR) when Steam's library list cannot be read.
  Only dinput8.dll and ts3online.ini in the game folder are written, and the DLL is only
  replaced or removed when it is ours.
#>
[CmdletBinding()]
param(
    [string]$GameDir = $env:TS3_DIR,
    [string]$Name = $env:USERNAME,
    [int]$Port = 27777,
    [switch]$Firewall,
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
$Game = 'Toy Story 3 Complete Edition'
$Exe = "$Game.exe"
$RuleName = 'ts3-online (UDP)'

function Find-GameDir {
    $steam = (Get-ItemProperty 'HKCU:\Software\Valve\Steam' -ErrorAction SilentlyContinue).SteamPath
    if (-not $steam) { throw 'Cannot find Steam. Pass -GameDir "<game folder>".' }
    $libs = @($steam)
    $vdf = Join-Path $steam 'steamapps\libraryfolders.vdf'
    if (Test-Path -LiteralPath $vdf -PathType Leaf) {
        foreach ($line in Get-Content -LiteralPath $vdf) {
            if ($line -match '^\s*"path"\s*"(.*)"') { $libs += ($Matches[1] -replace '\\\\', '\') }
        }
    }
    foreach ($lib in $libs) {
        $dir = Join-Path $lib "steamapps\common\$Game"
        if (Test-Path -LiteralPath (Join-Path $dir $Exe) -PathType Leaf) { return $dir }
    }
    throw "Cannot find $Game in the Steam libraries. Pass -GameDir `"<game folder>`"."
}

# Our DLL carries the ASCII marker "ts3online"; anything else named dinput8.dll is left alone.
function Test-OurDll([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $false }
    $text = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($Path))
    return $text.Contains('DirectInput8Create') -and $text.Contains('ts3online')
}

function Find-SourceDll {
    foreach ($p in @((Join-Path $PSScriptRoot 'dinput8.dll'), (Join-Path $PSScriptRoot '..\loader\target\release\dinput8.dll'))) {
        if (Test-Path -LiteralPath $p -PathType Leaf) { return (Resolve-Path -LiteralPath $p).Path }
    }
    throw 'No dinput8.dll next to this script or in loader\target\release. Build the loader first.'
}

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    return (New-Object Security.Principal.WindowsPrincipal $id).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not $GameDir) { $GameDir = Find-GameDir }
$dll = Join-Path $GameDir 'dinput8.dll'
$ini = Join-Path $GameDir 'ts3online.ini'
$exePath = Join-Path $GameDir $Exe
Write-Host "Game folder: $GameDir"

if ($Uninstall) {
    if (Test-OurDll $dll) {
        Remove-Item -LiteralPath $dll
        Write-Host 'Removed dinput8.dll.'
    } elseif (Test-Path -LiteralPath $dll) {
        Write-Host 'dinput8.dll there is not ours; left alone.'
    } else {
        Write-Host 'Not installed.'
    }
    if (Test-Path -LiteralPath $ini) { Write-Host "Kept $ini (delete it by hand if you want)." }
    if (Get-Command Get-NetFirewallRule -ErrorAction SilentlyContinue) {
        if (Get-NetFirewallRule -DisplayName $RuleName -ErrorAction SilentlyContinue) {
            if (Test-Admin) {
                Remove-NetFirewallRule -DisplayName $RuleName
                Write-Host 'Removed the firewall rule.'
            } else {
                Write-Host "Run as administrator to remove the firewall rule '$RuleName'."
            }
        }
    }
    return
}

if ((Test-Path -LiteralPath $dll) -and -not (Test-OurDll $dll)) {
    throw "$dll exists and is not ours (another mod?). Move it away first."
}
Copy-Item -LiteralPath (Find-SourceDll) -Destination $dll -Force
Write-Host 'Installed dinput8.dll.'

if (Test-Path -LiteralPath $ini) {
    Write-Host "Kept the existing $ini."
} else {
    $clean = ($Name -replace '[^\w\- ]', '').Trim()
    if (-not $clean) { $clean = 'player' }
    $lines = @(
        '# ts3-online: every player runs their own copy; the first copy to start hosts.',
        '# net=auto finds the other copies on your Tailscale tailnet (or list them in net_peers).',
        'net=auto',
        "name=$clean"
    )
    if ($Port -ne 27777) { $lines += "net_port=$Port" }
    [IO.File]::WriteAllLines($ini, $lines)
    Write-Host "Wrote $ini (net=auto, name=$clean)."
}

if ($Firewall) {
    if (-not (Test-Admin)) {
        Write-Host 'Run this script as administrator with -Firewall to add the rule, or allow the game when Windows asks.'
    } elseif (Get-NetFirewallRule -DisplayName $RuleName -ErrorAction SilentlyContinue) {
        Write-Host 'Firewall rule already present.'
    } else {
        New-NetFirewallRule -DisplayName $RuleName -Direction Inbound -Program $exePath -Protocol UDP -LocalPort $Port -Action Allow | Out-Null
        Write-Host "Added inbound UDP $Port for the game."
    }
}

Write-Host ''
Write-Host 'Back up %APPDATA%\Disney\Toy Story 3 Complete Edition before playing. Start the game from Steam.'
