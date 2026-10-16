# Screenshot the game window (scaled down) to a PNG.
#   powershell -File tools/shot.ps1 <out.png> [scale=3]
# Also prints foreground=True|False: drive virtual pads only while the game is in front.
param([Parameter(Mandatory = $true)][string]$Out, [int]$Scale = 3)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public class TsWin {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  public struct RECT { public int L, T, R, B; }
}
"@
[TsWin]::SetProcessDPIAware() | Out-Null
$p = Get-Process | Where-Object { $_.ProcessName -like 'Toy Story 3*' } | Select-Object -First 1
if (-not $p) { Write-Error "game is not running"; exit 1 }
$r = New-Object TsWin+RECT
[TsWin]::GetWindowRect($p.MainWindowHandle, [ref]$r) | Out-Null
$w = $r.R - $r.L; $h = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
$small = New-Object System.Drawing.Bitmap $bmp, ([int]($w / $Scale)), ([int]($h / $Scale))
$small.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose(); $small.Dispose()
Write-Output $Out
Write-Output ("foreground=" + ([TsWin]::GetForegroundWindow() -eq $p.MainWindowHandle))
