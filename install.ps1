# Termide Installer for Windows
# https://github.com/termide/termide
#
# Usage: irm https://raw.githubusercontent.com/termide/termide/main/install.ps1 | iex

$ErrorActionPreference = 'Stop'

$Repo = 'termide/termide'
$DefaultDir = if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA 'Programs\termide' } else { Join-Path $HOME '.termide' }
$InstallDir = if ($env:TERMIDE_INSTALL_DIR) { $env:TERMIDE_INSTALL_DIR } else { $DefaultDir }

# --- helpers ----------------------------------------------------------------

function Write-Info  { param([string]$Msg) Write-Host "> $Msg" -ForegroundColor Green }
function Write-Warn  { param([string]$Msg) Write-Host "! $Msg" -ForegroundColor Yellow }
function Write-Err   { param([string]$Msg) Write-Host "x $Msg" -ForegroundColor Red; exit 1 }

# --- detect system ----------------------------------------------------------

Write-Info 'Detecting system...'

$OSArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture

switch ($OSArch) {
    'X64' {
        $Target = 'x86_64-pc-windows-msvc'
    }
    'Arm64' {
        # Windows 11 on ARM64 can run x64 binaries through emulation.
        # Windows 10 ARM64 cannot — check the OS build number.
        $WinBuild = [System.Environment]::OSVersion.Version.Build
        if ($WinBuild -lt 22000) {
            Write-Err "ARM64 on Windows 10 (build $WinBuild) cannot run x64 binaries. Termide does not ship a native ARM64 build yet."
        }
        Write-Warn 'No native ARM64 build — installing the x64 build (runs under emulation on Windows 11).'
        $Target = 'x86_64-pc-windows-msvc'
    }
    default {
        Write-Err "Unsupported architecture: $OSArch. Termide ships x64 binaries only."
    }
}

Write-Info "Architecture: $OSArch"

# --- fetch latest version ---------------------------------------------------

Write-Info 'Fetching latest version...'

$RequestVersion = if ($env:TERMIDE_VERSION) { $env:TERMIDE_VERSION } else { $null }

if ($RequestVersion) {
    $Version = $RequestVersion
} else {
    try {
        $Release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
        $Version = $Release.tag_name -replace '^v', ''
    } catch {
        Write-Err "Failed to get latest version: $_"
    }
}

Write-Info "Version: $Version"

# --- download ---------------------------------------------------------------

$ArchiveName = "termide-$Version-$Target.zip"
$DownloadUrl = "https://github.com/$Repo/releases/download/$Version/$ArchiveName"
$TempDir     = Join-Path ([System.IO.Path]::GetTempPath()) "termide-install-$([System.Guid]::NewGuid().ToString('N').Substring(0,8))"
$ArchivePath = Join-Path $TempDir $ArchiveName

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null

Write-Info "Downloading $ArchiveName..."

try {
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ArchivePath -UseBasicParsing
} catch {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
    Write-Err "Download failed: $_"
}

# --- verify checksum (if available) -----------------------------------------

$ShaUrl = "$DownloadUrl.sha256"
try {
    $ShaResp = Invoke-WebRequest -Uri $ShaUrl -UseBasicParsing
    $ShaBody = ([System.Text.Encoding]::UTF8.GetString($ShaResp.Content)).Trim()
    # A valid checksum file is a 64-character hex string (optionally followed by
    # a filename). GitHub redirects missing release assets to an HTML 404 page
    # that is returned as 200 after the redirect, so reject anything that does
    # not look like a hash.
    $ExpectedHash = ($ShaBody -split '\s')[0]
    if ($ExpectedHash -match '^[0-9a-fA-F]{64}$') {
        $ActualHash = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash
        if ($ActualHash -ne $ExpectedHash.ToUpper()) {
            Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
            Write-Err "Checksum mismatch: expected $ExpectedHash, got $ActualHash"
        }
        Write-Info 'Checksum verified.'
    }
    # else: response is not a checksum (HTML 404 page, etc.) — skip.
} catch {
    # .sha256 file not published yet or unreachable — skip.
}

# --- check for running instance ---------------------------------------------

$ExePath = Join-Path $InstallDir 'termide.exe'

if (Test-Path $ExePath) {
    try {
        [System.IO.File]::Open($ExePath, 'Open', 'ReadWrite', 'None').Close()
    } catch {
        Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
        Write-Err "termide.exe is locked (probably running). Close termide and try again."
    }
}

# --- extract ----------------------------------------------------------------

Write-Info "Installing to $InstallDir..."

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

$ExtractDir = Join-Path $TempDir 'extracted'
Expand-Archive -Path $ArchivePath -DestinationPath $ExtractDir -Force

# The archive puts files at its root (termide.exe, README.md, LICENSE).
$Exe = Get-ChildItem -Path $ExtractDir -Filter 'termide.exe' -Recurse | Select-Object -First 1
if (-not $Exe) {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
    Write-Err 'termide.exe not found in the archive.'
}

Copy-Item -Path $Exe.FullName -Destination $ExePath -Force

# --- add to PATH ------------------------------------------------------------

# Persist in the user's PATH (Windows registry; skipped on other platforms).
if ($IsWindows -or (-not (Test-Path variable:IsWindows))) {
    $UserPath = [System.Environment]::GetEnvironmentVariable('Path', 'User')
    if ($UserPath -notlike "*$InstallDir*") {
        Write-Info 'Adding to PATH...'
        [System.Environment]::SetEnvironmentVariable('Path', "$InstallDir;$UserPath", 'User')
    }
}

# Also update the current session so the user can run termide right away.
if ($env:Path -notlike "*$InstallDir*") {
    $env:Path = "$InstallDir;$env:Path"
}

# --- done -------------------------------------------------------------------

Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue

Write-Host ''
Write-Info "Termide $Version installed successfully!"
Write-Info "Run 'termide' to start."
