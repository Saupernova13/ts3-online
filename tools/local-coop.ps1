<#
.SYNOPSIS
  Starts two copies of Toy Story 3 side by side on this PC, the first hosting and the second joined
  to it, for testing online co-op with two controllers.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File local-coop.ps1      (what LocalCoop.cmd runs)

.NOTES
  Needs the loader installed (Setup.cmd) and two controllers: copy 1 plays with controller 1,
  copy 2 with controller 2. Copy 1 hosts with your own save; copy 2 downloads it and never writes it.
  While the games run, ts3online.ini in the game folder holds a two-copy setup; your own file is kept
  as ts3online.ini.before-local-coop and put back when both games have closed (or on the next run,
  if this window was closed first).
#>
[CmdletBinding()]
param(
    [string]$GameDir = $env:TS3_DIR,
    [int]$Port = 27777
)
$ErrorActionPreference = 'Stop'
$Game = 'Toy Story 3 Complete Edition'
$Exe = "$Game.exe"
$AppId = 4049180

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
        $dir = [IO.Path]::Combine($lib, "steamapps\common\$Game")
        if (Test-Path -LiteralPath ([IO.Path]::Combine($dir, $Exe)) -PathType Leaf) { return $dir }
    }
    throw "Cannot find $Game in the Steam libraries. Pass -GameDir `"<game folder>`"."
}

function Get-Games { @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -eq $Game }) }

# Half the primary screen's work area per copy, 16:9, centred vertically, in physical pixels.
function Get-Halves {
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -Namespace LocalCoop -Name Dpi -MemberDefinition '[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();'
    [void][LocalCoop.Dpi]::SetProcessDPIAware()
    $a = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    $w = [int][Math]::Floor($a.Width / 2)
    $h = [int][Math]::Min($a.Height, [Math]::Round($w * 9 / 16))
    $y = $a.Top + [int](($a.Height - $h) / 2)
    return @("$($a.Left),$y,$w,$h", "$($a.Left + $w),$y,$w,$h")
}

function Restore-Ini([string]$Ini) {
    $backup = "$Ini.before-local-coop"
    $none = "$Ini.none-before-local-coop"
    if (Test-Path -LiteralPath $backup) {
        Move-Item -LiteralPath $backup -Destination $Ini -Force
        Write-Host 'Put your ts3online.ini back.'
    } elseif (Test-Path -LiteralPath $none) {
        Remove-Item -LiteralPath $Ini -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $none
        Write-Host 'Removed the two-copy ts3online.ini.'
    }
}

function Wait-For([scriptblock]$Done, [int]$Seconds, [string]$What) {
    $end = (Get-Date).AddSeconds($Seconds)
    while (-not (& $Done)) {
        if ((Get-Date) -gt $end) { throw "Timed out waiting for $What." }
        Start-Sleep -Milliseconds 500
    }
}

if (-not $GameDir) { $GameDir = Find-GameDir }
$ini = Join-Path $GameDir 'ts3online.ini'
if (-not (Test-Path -LiteralPath (Join-Path $GameDir 'dinput8.dll'))) { throw "The loader is not installed in $GameDir. Run Setup.cmd first." }
if ((Get-Games).Count -gt 0) {
    Write-Host 'Toy Story 3 is already running. Close it, then run this again.'
    return
}
Restore-Ini $ini  # a run whose window was closed early left the two-copy file behind

$halves = Get-Halves
if (Test-Path -LiteralPath $ini) { Copy-Item -LiteralPath $ini -Destination "$ini.before-local-coop" } else { New-Item -ItemType File -Path "$ini.none-before-local-coop" | Out-Null }
$lines = @(
    '# written by local-coop.ps1; your own file comes back when both games close',
    'enable=instances',
    '[copy1]', 'pad=1', "window=$($halves[0])", "net=host:$Port", 'name=Player 1',
    '[copy2]', 'pad=2', "window=$($halves[1])", "net=client:127.0.0.1:$Port", 'name=Player 2'
)
[IO.File]::WriteAllLines($ini, $lines)

try {
    $log1 = Join-Path $GameDir 'ts3online.log'
    $started = Get-Date
    $steamExe = (Get-ItemProperty 'HKCU:\Software\Valve\Steam' -ErrorAction SilentlyContinue).SteamExe
    if (-not $steamExe) { throw 'Cannot find Steam to start the first copy.' }
    Write-Host 'Starting copy 1 (host, controller 1) through Steam...'
    & $steamExe -applaunch $AppId
    Wait-For { (Get-Games).Count -ge 1 } 120 'copy 1 to start'
    Wait-For { (Test-Path -LiteralPath $log1) -and (Get-Item -LiteralPath $log1).LastWriteTime -gt $started -and (Select-String -LiteralPath $log1 -Pattern 'net: host on' -Quiet) } 60 'copy 1 to host'
    Write-Host 'Starting copy 2 (joins copy 1, controller 2)...'
    & explorer.exe (Join-Path $GameDir $Exe)
    Wait-For { (Get-Games).Count -ge 2 } 120 'copy 2 to start'
    Write-Host ''
    Write-Host 'Both copies are running. On each: Start Game, Load Game, then enter Toy Box,'
    Write-Host 'copy 1 first, and pick different characters. Keep this window open: when both'
    Write-Host 'games are closed it puts your ts3online.ini back.'
    while ((Get-Games).Count -gt 0) { Start-Sleep -Seconds 2 }
} finally {
    Restore-Ini $ini
}
