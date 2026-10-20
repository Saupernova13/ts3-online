<#
.SYNOPSIS
  Installs or removes the ts3-online loader (dinput8.dll) in the Steam copy of Toy Story 3,
  and writes a starter ts3online.ini.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File install.ps1 -Name Alice
  powershell -ExecutionPolicy Bypass -File install.ps1 -Firewall
  powershell -ExecutionPolicy Bypass -File install.ps1 -Name Alice -Password "our secret"
  powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall
  powershell -ExecutionPolicy Bypass -File install.ps1 -Setup     (what Setup.cmd runs)
  powershell -ExecutionPolicy Bypass -File install.ps1 -Update    (what Update.cmd runs)

.NOTES
  DLL source: next to this script (release zip), else loader\target\release.
  Set -GameDir (or TS3_DIR) if Steam's library list cannot be read.
  In the game folder it writes only dinput8.dll (when ours) and ts3online.ini; -Setup also backs
  up saves under Documents, and -Update refreshes the scripts in this folder.
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
    [switch]$FirewallOnly,
    # install the latest GitHub release's DLL (and refresh these scripts)
    [switch]$Update
)
$ErrorActionPreference = 'Stop'
$Game = 'Toy Story 3 Complete Edition'
$Exe = "$Game.exe"
$RuleName = 'ts3-online (UDP)'
$Repo = 'Saupernova13/ts3-online'

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
        # not Join-Path: it throws for a library on an unplugged drive
        $dir = [IO.Path]::Combine($lib, "steamapps\common\$Game")
        if (Test-Path -LiteralPath ([IO.Path]::Combine($dir, $Exe)) -PathType Leaf) { return $dir }
    }
    throw "Cannot find $Game in the Steam libraries. Pass -GameDir `"<game folder>`"."
}

function Test-OurDll([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $false }
    $text = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($Path))
    return $text.Contains('DirectInput8Create') -and $text.Contains('ts3online')
}

function Get-DllVersion([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    $text = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($Path))
    if ($text -match 'ts3online-version=(\d+\.\d+\.\d+);') { return $Matches[1] }
    return $null
}

function Update-Loader {
    if ((Test-Path -LiteralPath $dll) -and -not (Test-OurDll $dll)) { throw "$dll is not ours (another mod?). Move it away first." }
    $installed = Get-DllVersion $dll
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $headers = @{ 'User-Agent' = 'ts3-online-update' }
    $rel = Invoke-RestMethod -UseBasicParsing -Headers $headers -Uri "https://api.github.com/repos/$Repo/releases/latest"
    $latest = ([string]$rel.tag_name).TrimStart('v')
    $shown = if ($installed) { $installed } elseif (Test-Path -LiteralPath $dll) { 'an older release' } else { 'nothing' }
    Write-Host "Installed: $shown. Latest release: $latest."
    if ($installed -and ([version]$installed -ge [version]$latest)) { Write-Host 'Already up to date.'; return }
    if (Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -eq $Game }) { throw 'Close Toy Story 3 first, then run Update again.' }
    $asset = @($rel.assets) | Where-Object { $_.name -eq 'ts3-online.zip' } | Select-Object -First 1
    if (-not $asset) { throw "Release $latest has no ts3-online.zip." }
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('ts3-online-update-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $zip = Join-Path $tmp 'ts3-online.zip'
        Write-Host "Downloading $($asset.browser_download_url)"
        Invoke-WebRequest -UseBasicParsing -Headers $headers -Uri $asset.browser_download_url -OutFile $zip
        Expand-Archive -LiteralPath $zip -DestinationPath $tmp
        $new = Join-Path $tmp 'ts3-online'
        if (-not (Test-OurDll (Join-Path $new 'dinput8.dll'))) { throw 'The download does not contain the ts3-online dinput8.dll.' }
        Copy-Item -LiteralPath (Join-Path $new 'dinput8.dll') -Destination $dll -Force
        Write-Host "Installed ts3-online $latest. Your ts3online.ini and saves are unchanged."
        # a source checkout keeps its own scripts
        if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot '..\loader\Cargo.toml'))) {
            Copy-Item -Path (Join-Path $new '*') -Destination $PSScriptRoot -Recurse -Force
            Write-Host "Updated the files in $PSScriptRoot."
        }
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (-not (Test-Path -LiteralPath $ini)) { Write-Host 'No ts3online.ini yet: run Setup.cmd to finish setting up.' }
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

function Backup-Saves {
    $src = Join-Path $env:APPDATA "Disney\$Game"
    if (-not (Test-Path -LiteralPath $src)) { Write-Host 'No saves yet; nothing to back up.'; return }
    $dst = Join-Path ([Environment]::GetFolderPath('MyDocuments')) ('ts3-online save backup\' + (Get-Date -Format 'yyyy-MM-dd'))
    if (Test-Path -LiteralPath $dst) { Write-Host "Saves already backed up today: $dst"; return }
    New-Item -ItemType Directory -Force -Path $dst | Out-Null
    Copy-Item -LiteralPath $src -Destination $dst -Recurse
    Write-Host "Backed up your saves to $dst"
}

# Online play needs Tailscale unless both PCs share a LAN.
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
        if ($peers -eq 0) { Write-Host '  No other device is online: your friend must join your tailnet, or you share this PC with them (see README).' }
    } else {
        Write-Host 'Tailscale: installed but not connected. Open Tailscale and sign in.'
    }
}

if (-not $GameDir) { $GameDir = Find-GameDir }
$dll = Join-Path $GameDir 'dinput8.dll'
$ini = Join-Path $GameDir 'ts3online.ini'
$exePath = Join-Path $GameDir $Exe
Write-Host "Game folder: $GameDir"

if ($Update) {
    Update-Loader
    return
}

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
