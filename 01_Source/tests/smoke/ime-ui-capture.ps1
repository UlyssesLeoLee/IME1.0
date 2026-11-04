# Takes a picture of the desktop while the IME window probe keeps its windows open.
#   powershell -ExecutionPolicy Bypass -File tests/smoke/ime-ui-capture.ps1 -Out <file.png>
param(
  [string]$Out = "E:\IME1.0\02_Verification\ime-ui\ime-windows.png"
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bmp)
$graphics.CopyFromScreen($bounds.X, $bounds.Y, 0, 0, $bmp.Size)
$graphics.Dispose()
$folder = Split-Path -Parent $Out
if ($folder -and -not (Test-Path $folder)) { New-Item -ItemType Directory -Force -Path $folder | Out-Null }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
$info = Get-Item $Out
Write-Output ("{0} {1} bytes" -f $Out, $info.Length)
