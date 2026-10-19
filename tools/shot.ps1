# Screenshot the game window (scaled down) to a PNG.
#   powershell -File tools/shot.ps1 <out.png> [scale=3] [-ProcessId <pid>]
# Also prints foreground=True|False (true when any copy of the game is in front): drive virtual pads
# only while the game is in front. With several copies running, -ProcessId picks the window to capture.
param([Parameter(Mandatory = $true)][string]$Out, [int]$Scale = 3, [int]$ProcessId = 0)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public class TsWin {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  public struct RECT { public int L, T, R, B; }
}
"@
[TsWin]::SetProcessDPIAware() | Out-Null
$all = @(Get-Process | Where-Object { $_.ProcessName -like 'Toy Story 3*' })
$p = if ($ProcessId) { $all | Where-Object { $_.Id -eq $ProcessId } } else { $all | Select-Object -First 1 }
if (-not $p) { Write-Error "game is not running"; exit 1 }
$r = New-Object TsWin+RECT
[TsWin]::GetWindowRect($p.MainWindowHandle, [ref]$r) | Out-Null
$w = $r.R - $r.L; $h = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
# PrintWindow with PW_RENDERFULLCONTENT captures the window even when another window covers it
$hdc = $g.GetHdc()
$ok = [TsWin]::PrintWindow($p.MainWindowHandle, $hdc, 2)
$g.ReleaseHdc($hdc)
if (-not $ok) { $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size) }
$small = New-Object System.Drawing.Bitmap $bmp, ([int]($w / $Scale)), ([int]($h / $Scale))
$small.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose(); $small.Dispose()
Write-Output $Out
$fg = [TsWin]::GetForegroundWindow()
$front = $all | Where-Object { $_.MainWindowHandle -eq $fg } | Select-Object -First 1
Write-Output ("foreground=" + [bool]$front)
if ($front) { Write-Output ("foreground_pid=" + $front.Id) }
