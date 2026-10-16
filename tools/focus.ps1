# Bring the game window to the front (a tapped Alt lets a background process take focus).
#   powershell -File tools/focus.ps1 [-Escape]    -Escape first dismisses an open Start menu
param([switch]$Escape)
Add-Type @"
using System; using System.Runtime.InteropServices;
public class TsFg {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
}
"@
$p = Get-Process | Where-Object { $_.ProcessName -like 'Toy Story 3*' } | Select-Object -First 1
if (-not $p) { Write-Error "game is not running"; exit 1 }
if ($Escape) {
  [TsFg]::keybd_event(0x1B, 0, 0, [UIntPtr]::Zero); [TsFg]::keybd_event(0x1B, 0, 2, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 400
}
[TsFg]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero); [TsFg]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)
[TsFg]::ShowWindow($p.MainWindowHandle, 9) | Out-Null
[TsFg]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
Start-Sleep -Milliseconds 300
Write-Output ("foreground=" + ([TsFg]::GetForegroundWindow() -eq $p.MainWindowHandle))
