<#
launch-windows: start an isolated copy of a build — its own folder and its own clecta-data, so
nothing of yours is read or written — in a window of fixed size (1180x960, from the seeded
settings), so a point measured on one screenshot is the same point the next time. For any
check of the real app, by hand or by script; `smoke-windows.ps1` is one.

Returns the process id and the folder, and leaves the app running. Stop it by that process id,
then delete the folder:
	Stop-Process -Id <Pid>; Remove-Item -LiteralPath <Folder> -Recurse -Force

Usage, from desktop/, after `cargo build`:
	powershell -NoProfile -ExecutionPolicy Bypass -File scripts\launch-windows.ps1
	... -inFiles 300		# the files pane opens on 300 empty .mp3 files, for scrolling
	... -inFolder D:\music	# or on a folder of your own, which is only read
#>
param(
	[string] $inExe = "$PSScriptRoot\..\target\debug\clecta.exe",	# the build to launch
	[string] $inFolder = '',	# the folder the files pane opens on; a new, empty one when not given
	[int] $inFiles = 0			# empty .mp3 files to put in that new folder
)

$ErrorActionPreference = 'Stop'

$vDir = Join-Path $env:TEMP ('clecta-launch-' + [guid]::NewGuid().ToString('N'))	# the isolated copy
New-Item -ItemType Directory -Path (Join-Path $vDir 'clecta-data') | Out-Null
if (-not $inFolder) {
	$inFolder = Join-Path $vDir 'music'
	New-Item -ItemType Directory -Path $inFolder | Out-Null
	for ($vIndex = 1; $vIndex -le $inFiles; $vIndex++) {
		Set-Content -LiteralPath (Join-Path $inFolder ('track {0:D3}.mp3' -f $vIndex)) -Value ''
	}
}
Copy-Item -LiteralPath $inExe -Destination (Join-Path $vDir 'clecta.exe')
@{ window = @(1180.0, 960.0); decks_height = 480.0; folder = $inFolder } |
	ConvertTo-Json -Compress |
	Set-Content -Encoding ascii -LiteralPath (Join-Path $vDir 'clecta-data\settings.json')

$vProcess = Start-Process -FilePath (Join-Path $vDir 'clecta.exe') -WorkingDirectory $vDir -PassThru	# the app
[pscustomobject]@{ Pid = $vProcess.Id; Folder = $vDir }
