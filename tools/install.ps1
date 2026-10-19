<#
.SYNOPSIS
  Installs or removes the ts3-online loader (dinput8.dll) for the Steam version of
  Toy Story 3 Complete Edition, and writes a starter ts3online.ini for online play.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File install.ps1 -Name Alice
  powershell -ExecutionPolicy Bypass -File install.ps1 -Firewall
  powershell -ExecutionPolicy Bypass -File install.ps1 -Name Alice -Password "our secret"
  powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall
  powershell -ExecutionPolicy Bypass -File install.ps1 -Setup     (what Setup.cmd runs)

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
    [string]$Password,
    [switch]$Firewall,
    [switch]$Uninstall,
    # guided install: back up the saves, check Tailscale, offer the firewall rule
    [switch]$Setup,
    # only add the firewall rule (Setup relaunches itself elevated with this)
    [switch]$FirewallOnly
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
        # [IO.Path]::Combine, not Join-Path: a library on an unplugged drive must not throw
        $dir = [IO.Path]::Combine($lib, "steamapps\common\$Game")
        if (Test-Path -LiteralPath ([IO.Path]::Combine($dir, $Exe)) -PathType Leaf) { return $dir }
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

function Add-FirewallRule([string]$ExePath) {
    if (Get-NetFirewallRule -DisplayName $RuleName -ErrorAction SilentlyContinue) {
        Write-Host 'Firewall rule already present.'
    } else {
        New-NetFirewallRule -DisplayName $RuleName -Direction Inbound -Program $ExePath -Protocol UDP -LocalPort $Port -Action Allow | Out-Null
        Write-Host "Added inbound UDP $Port for the game."
    }
}

# Copies the game's save folder to Documents\ts3-online save backup\<date>, once per day.
function Backup-Saves {
    $src = Join-Path $env:APPDATA "Disney\$Game"
    if (-not (Test-Path -LiteralPath $src)) { Write-Host 'No saves yet; nothing to back up.'; return }
    $dst = Join-Path ([Environment]::GetFolderPath('MyDocuments')) ('ts3-online save backup\' + (Get-Date -Format 'yyyy-MM-dd'))
    if (Test-Path -LiteralPath $dst) { Write-Host "Saves already backed up today: $dst"; return }
    New-Item -ItemType Directory -Force -Path $dst | Out-Null
    Copy-Item -LiteralPath $src -Destination $dst -Recurse
    Write-Host "Backed up your saves to $dst"
}

# Says whether Tailscale is installed and connected; online play needs it unless both PCs share a LAN.
function Show-Tailscale {
    $exe = (Get-Command tailscale -ErrorAction SilentlyContinue).Source
    if (-not $exe) {
        $pf = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'
        if (Test-Path -LiteralPath $pf) { $exe = $pf }
    }
    if (-not $exe) {
        Write-Host 'Tailscale: not installed. Install it from https://tailscale.com/download and sign in with the'
        Write-Host '  same account as your friend (or share your device with them), unless you are on the same LAN.'
        return
    }
    try { $st = & $exe status --json 2>$null | ConvertFrom-Json } catch { $st = $null }
    if ($st -and $st.BackendState -eq 'Running') {
        $peers = @($st.Peer.PSObject.Properties | Where-Object { $_.Value.Online }).Count
        Write-Host ("Tailscale: connected as {0} ({1}), {2} other device(s) online." -f $st.Self.HostName, ($st.Self.TailscaleIPs | Select-Object -First 1), $peers)
    } else {
        Write-Host 'Tailscale: installed but not connected. Open Tailscale and sign in.'
    }
}

if (-not $GameDir) { $GameDir = Find-GameDir }
$dll = Join-Path $GameDir 'dinput8.dll'
$ini = Join-Path $GameDir 'ts3online.ini'
$exePath = Join-Path $GameDir $Exe
Write-Host "Game folder: $GameDir"

if ($FirewallOnly) {
    Add-FirewallRule $exePath
    return
}

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

if ($Setup) { Backup-Saves }

if ((Test-Path -LiteralPath $dll) -and -not (Test-OurDll $dll)) {
    throw "$dll exists and is not ours (another mod?). Move it away first."
}
Copy-Item -LiteralPath (Find-SourceDll) -Destination $dll -Force
Write-Host 'Installed dinput8.dll.'

if (Test-Path -LiteralPath $ini) {
    Write-Host "Kept the existing $ini."
    if ($Password) { Write-Host 'The password was not written: add net_key=<text> to that file by hand.' }
} else {
    $clean = ($Name -replace '[^\w\- ]', '').Trim()
    if (-not $clean) { $clean = 'player' }
    $lines = @(
        '# ts3-online: every player runs their own copy. net=menu shows a window at start that lists the',
        '# games on your Tailscale tailnet or LAN: pick one and Join, or Host. (net=auto skips the window.)',
        'net=menu',
        "name=$clean"
    )
    if ($Port -ne 27777) { $lines += "net_port=$Port" }
    $key = if ($Password) { $Password.Trim() } else { '' }
    if ($key) { $lines += "net_key=$key" }
    [IO.File]::WriteAllLines($ini, $lines)
    Write-Host "Wrote $ini (net=menu, name=$clean)."
    if ($key) { Write-Host 'Password set: every player must use the same password (net_key in ts3online.ini).' }
}

if ($Firewall) {
    if (-not (Test-Admin)) {
        Write-Host 'Run this script as administrator with -Firewall to add the rule, or allow the game when Windows asks.'
    } else {
        Add-FirewallRule $exePath
    }
}

if ($Setup) {
    Show-Tailscale
    $hasRule = [bool](Get-NetFirewallRule -DisplayName $RuleName -ErrorAction SilentlyContinue)
    if (-not $hasRule) {
        $answer = Read-Host 'Let other players reach your game through the Windows Firewall? Needs administrator rights. [Y/n]'
        if ($answer -notmatch '^[nN]') {
            if (Test-Admin) {
                Add-FirewallRule $exePath
            } else {
                $elevated = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-FirewallOnly', '-GameDir', "`"$GameDir`"", '-Port', $Port)
                try {
                    Start-Process powershell -Verb RunAs -Wait -ArgumentList $elevated
                    if (Get-NetFirewallRule -DisplayName $RuleName -ErrorAction SilentlyContinue) { Write-Host 'Firewall rule added.' }
                } catch {
                    Write-Host 'Skipped the firewall rule; allow the game when Windows asks the first time.'
                }
            }
        }
    }
    Write-Host ''
    Write-Host 'Done. Start Toy Story 3 from Steam: a "Toy Story 3 Online" window lets you set your name and'
    Write-Host 'password, and Host a game or Join one from the list.'
    return
}

Write-Host ''
Write-Host 'Back up %APPDATA%\Disney\Toy Story 3 Complete Edition before playing. Start the game from Steam.'
