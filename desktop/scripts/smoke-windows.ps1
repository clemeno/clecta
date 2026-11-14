<#
smoke-windows: does a confirmation box appear on Windows, or does the app hang behind a hidden one?

Launches an isolated copy of a build (its own folder and its own clecta-data, so nothing of
yours is read or written), presses Clear cache, and asks Windows whether the "Clear the cache"
box is visible. Then cancels it and checks the app is idle again. All four confirmations go
through the same `ask` (PLAN Q70), so this one box stands for them all.

Exit 0 green: the box appeared and the app came back. Exit 1 red: the box was hidden, or the
app kept a core busy after it. Exit 2: no box at all, which usually means the click missed.

The click lands on a measured spot in a window of fixed size (1180x960, from the seeded
settings). A layout change that moves the button means re-measuring `$kClearCache`.

Usage, from desktop/, after `cargo build` (add `-inShot shot.png` to see where it clicks):
	powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-windows.ps1
#>
param(
	[string] $inExe = "$PSScriptRoot\..\target\debug\clecta.exe",	# the build under test
	[string] $inShot = ''	# when set, a PNG of the window just before the click, for re-measuring
)

$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

// The Win32 calls the smoke test needs: find the app's windows, click in one, answer a box.
public static class SmokeNative
{
	public delegate bool EnumProc(IntPtr inHwnd, IntPtr inParam);	// EnumWindows callback

	[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
	[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc inProc, IntPtr inParam);
	[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr inHwnd, out uint outPid);
	[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr inHwnd);
	[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr inHwnd, out Rect outRect);
	[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr inHwnd, StringBuilder inText, int inMax);
	[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr inHwnd, StringBuilder inText, int inMax);
	[DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr inHwnd, ref Point ioPoint);
	[DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr inHwnd);
	[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr inHwnd);
	[DllImport("user32.dll")] public static extern bool SetCursorPos(int inX, int inY);
	[DllImport("user32.dll")] public static extern void mouse_event(uint inFlags, uint inX, uint inY, uint inData, UIntPtr inExtra);
	[DllImport("user32.dll")] public static extern bool PostMessage(IntPtr inHwnd, uint inMsg, IntPtr inW, IntPtr inL);

	// A window's screen rectangle, as GetWindowRect fills it.
	public struct Rect { public int Left, Top, Right, Bottom; }

	// A point in screen or client pixels.
	public struct Point { public int X, Y; }

	// Every top-level window of a process, visible or not.
	public static List<IntPtr> WindowsOf(uint inPid)
	{
		var vFound = new List<IntPtr>();	// matching windows
		EnumWindows((vHwnd, vParam) =>
		{
			uint vPid;	// the window's owning process
			GetWindowThreadProcessId(vHwnd, out vPid);
			if (vPid == inPid) vFound.Add(vHwnd);
			return true;
		}, IntPtr.Zero);
		return vFound;
	}

	// The process's largest visible window: the app's, not winit's 16x16 event target, which
	// is visible from the start. Zero until a window of at least 100x100 exists.
	public static IntPtr MainOf(uint inPid)
	{
		IntPtr vBest = IntPtr.Zero;	// largest window found so far
		long vBestArea = 100 * 100;	// its area, in pixels; the floor a real window clears
		foreach (var vHwnd in WindowsOf(inPid))
		{
			Rect vRect;	// the window's screen rectangle
			if (!IsWindowVisible(vHwnd) || !GetWindowRect(vHwnd, out vRect)) continue;
			long vArea = (long)(vRect.Right - vRect.Left) * (vRect.Bottom - vRect.Top);	// this window's area
			if (vArea > vBestArea) { vBestArea = vArea; vBest = vHwnd; }
		}
		return vBest;
	}

	// A window's class name, or its title.
	public static string Text(IntPtr inHwnd, bool inClass)
	{
		var vBuffer = new StringBuilder(256);	// receives the UTF-16 text
		if (inClass) GetClassName(inHwnd, vBuffer, vBuffer.Capacity);
		else GetWindowText(inHwnd, vBuffer, vBuffer.Capacity);
		return vBuffer.ToString();
	}
}
"@

$kClearCache = @(601, 566)	# the Clear cache button, in logical client pixels of a 1180x960 window
$kTitle = 'Clear the cache'	# the confirmation box's title, from app.rs
$kCommand = 0x0111			# WM_COMMAND
$kCancel = 2				# IDCANCEL
$kLeftDown = 0x0002			# MOUSEEVENTF_LEFTDOWN
$kLeftUp = 0x0004			# MOUSEEVENTF_LEFTUP

# The confirmation box of a process, or $null.
function Find-Box([uint32] $inPid) {
	foreach ($vHwnd in [SmokeNative]::WindowsOf($inPid)) {
		if ([SmokeNative]::Text($vHwnd, $true) -eq '#32770' -and [SmokeNative]::Text($vHwnd, $false) -eq $kTitle) {
			return $vHwnd
		}
	}
	return $null
}

# CPU milliseconds a process spends over two seconds.
function Measure-Cpu([System.Diagnostics.Process] $inProcess) {
	$vBefore = $inProcess.TotalProcessorTime.TotalMilliseconds	# CPU time at the start
	Start-Sleep -Seconds 2
	$inProcess.Refresh()
	return $inProcess.TotalProcessorTime.TotalMilliseconds - $vBefore
}

[void][SmokeNative]::SetProcessDPIAware()	# physical pixels throughout

$vLaunch = & (Join-Path $PSScriptRoot 'launch-windows.ps1') -inExe $inExe	# the isolated copy, running
$vDir = $vLaunch.Folder						# its folder, deleted at the end
$vProcess = Get-Process -Id $vLaunch.Pid	# the app under test
$vVerdict = 1	# the exit code, red until proven otherwise
try {
	$vMain = [IntPtr]::Zero	# the app's window, once it exists
	for ($vTry = 0; $vTry -lt 50 -and $vMain -eq [IntPtr]::Zero; $vTry++) {
		Start-Sleep -Milliseconds 200
		$vMain = [SmokeNative]::MainOf([uint32]$vProcess.Id)
	}
	if ($vMain -eq [IntPtr]::Zero) { throw 'the app never showed a window' }
	Start-Sleep -Seconds 2	# let the first layout settle

	$vScale = [SmokeNative]::GetDpiForWindow($vMain) / 96.0	# physical pixels per logical one
	$vOrigin = New-Object SmokeNative+Point					# the client area's top-left, on screen
	[void][SmokeNative]::ClientToScreen($vMain, [ref]$vOrigin)
	[void][SmokeNative]::SetForegroundWindow($vMain)
	Start-Sleep -Milliseconds 200
	if ($inShot) {
		Add-Type -AssemblyName System.Drawing
		$vRect = New-Object SmokeNative+Rect	# the window's screen rectangle
		[void][SmokeNative]::GetWindowRect($vMain, [ref]$vRect)
		$vBitmap = New-Object System.Drawing.Bitmap ($vRect.Right - $vRect.Left), ($vRect.Bottom - $vRect.Top)	# the capture
		[System.Drawing.Graphics]::FromImage($vBitmap).CopyFromScreen($vRect.Left, $vRect.Top, 0, 0, $vBitmap.Size)
		$vBitmap.Save($inShot, [System.Drawing.Imaging.ImageFormat]::Png)
		Write-Output ('shot: window at ({0},{1}), client origin at ({2},{3}), scale {4}' -f $vRect.Left, $vRect.Top, $vOrigin.X, $vOrigin.Y, $vScale)
	}
	[void][SmokeNative]::SetCursorPos($vOrigin.X + [int]($kClearCache[0] * $vScale), $vOrigin.Y + [int]($kClearCache[1] * $vScale))
	Start-Sleep -Milliseconds 100
	[SmokeNative]::mouse_event($kLeftDown, 0, 0, 0, [UIntPtr]::Zero)
	Start-Sleep -Milliseconds 50
	[SmokeNative]::mouse_event($kLeftUp, 0, 0, 0, [UIntPtr]::Zero)

	$vBox = $null	# the confirmation box, once it exists
	for ($vTry = 0; $vTry -lt 15 -and -not $vBox; $vTry++) {
		Start-Sleep -Milliseconds 200
		$vBox = Find-Box ([uint32]$vProcess.Id)
	}
	if (-not $vBox) {
		Write-Output 'NO BOX: the click found no Clear cache button; re-measure $kClearCache'
		$vVerdict = 2
		return
	}
	Start-Sleep -Milliseconds 500	# a box shows itself once its loop goes idle
	if (-not [SmokeNative]::IsWindowVisible($vBox)) {
		Write-Output ('RED: the box exists but is hidden; the app burns {0:N0} ms of CPU in 2 s' -f (Measure-Cpu $vProcess))
		return
	}

	[void][SmokeNative]::PostMessage($vBox, $kCommand, [IntPtr]$kCancel, [IntPtr]::Zero)
	Start-Sleep -Seconds 1
	$vBusy = Measure-Cpu $vProcess	# CPU after the box closed
	if (Find-Box ([uint32]$vProcess.Id)) {
		Write-Output 'RED: the box did not close on Cancel'
	} elseif ($vBusy -gt 1000) {
		Write-Output ('RED: the app burns {0:N0} ms of CPU in 2 s after the box closed' -f $vBusy)
	} else {
		Write-Output ('GREEN: the box appeared, Cancel closed it, the app is idle ({0:N0} ms of CPU in 2 s)' -f $vBusy)
		$vVerdict = 0
	}
}
finally {
	Stop-Process -Id $vProcess.Id -Force -ErrorAction SilentlyContinue
	$vProcess.WaitForExit(5000) | Out-Null
	Remove-Item -LiteralPath $vDir -Recurse -Force -ErrorAction SilentlyContinue
	exit $vVerdict
}
