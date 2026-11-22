# Termide Installer for Windows
# https://github.com/termide/termide
#
# Usage: irm https://raw.githubusercontent.com/termide/termide/main/install.ps1 | iex

# `iex` runs the script in the caller's session: the script block keeps its
# variables and $ErrorActionPreference out of it, and a failure is thrown and
# reported instead of `exit`, which would close the user's terminal.
& {

$ErrorActionPreference = 'Stop'
# Windows PowerShell 5.1 redraws the Invoke-WebRequest progress bar per chunk,
# which slows the download many times over.
$ProgressPreference = 'SilentlyContinue'

$Repo = 'termide/termide'
$DefaultDir = if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA 'Programs\termide' } else { Join-Path $HOME '.termide' }
$InstallDir = if ($env:TERMIDE_INSTALL_DIR) { $env:TERMIDE_INSTALL_DIR } else { $DefaultDir }
$TempDir    = $null

# --- helpers ----------------------------------------------------------------

function Write-Info  { param([string]$Msg) Write-Host "> $Msg" -ForegroundColor Green }
function Write-Warn  { param([string]$Msg) Write-Host "! $Msg" -ForegroundColor Yellow }
function Write-Err   { param([string]$Msg) throw $Msg }

# The machine's own architecture. Windows PowerShell 5.1 runs on .NET
# Framework, where RuntimeInformation.OSArchitecture can come back empty, and
# an x64 process under emulation sees PROCESSOR_ARCHITECTURE=AMD64; the
# system environment in the registry names the native one either way.
function Get-OSArch {
    $Native = (Get-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment' -ErrorAction SilentlyContinue).PROCESSOR_ARCHITECTURE
    if (-not $Native) { $Native = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE } }
    switch ($Native) {
        'AMD64' { 'X64' }
        'ARM64' { 'Arm64' }
        default { "$Native" }
    }
}

try {

# --- detect system ----------------------------------------------------------

Write-Info 'Detecting system...'

$OSArch = Get-OSArch

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
        Write-Warn 'No native ARM64 build yet: installing the x64 one, which Windows 11 runs under emulation.'
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
    # Tags carry no `v`: TERMIDE_VERSION=v0.51.0 means 0.51.0.
    $Version = $RequestVersion -replace '^v', ''
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
    Write-Err "Download failed: $_"
}

# --- verify checksum ---------------------------------------------------------

$ShaUrl = "$DownloadUrl.sha256"
try {
    $ShaResp = Invoke-WebRequest -Uri $ShaUrl -UseBasicParsing
} catch {
    Write-Err "No checksum published at ${ShaUrl}: not installing an unverified download"
}
# Windows PowerShell gives an asset served as a file in bytes, PowerShell 7
# a text one as a string.
$ShaContent = $ShaResp.Content
$ShaBody = if ($ShaContent -is [byte[]]) { [System.Text.Encoding]::UTF8.GetString($ShaContent) } else { [string]$ShaContent }
# A 64-character hex string, optionally followed by a file name; anything
# else (an HTML error page) is no checksum.
$ExpectedHash = ($ShaBody.Trim() -split '\s')[0]
if ($ExpectedHash -notmatch '^[0-9a-fA-F]{64}$') {
    Write-Err "The checksum at $ShaUrl is not a SHA-256"
}
$ActualHash = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash
if ($ActualHash -ne $ExpectedHash.ToUpper()) {
    Write-Err "Checksum mismatch: expected $ExpectedHash, got $ActualHash"
}
Write-Info 'Checksum verified.'

# --- check for running instance ---------------------------------------------

$ExePath = Join-Path $InstallDir 'termide.exe'

if (Test-Path $ExePath) {
    try {
        [System.IO.File]::Open($ExePath, 'Open', 'ReadWrite', 'None').Close()
    } catch {
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
    Write-Err 'termide.exe not found in the archive.'
}

Copy-Item -Path $Exe.FullName -Destination $ExePath -Force

# --- add to PATH ------------------------------------------------------------

# Whether the `;`-separated $PathList has $Dir as one of its entries, not
# merely inside one (C:\Tools\termide-old holds C:\Tools\termide).
function Test-PathEntry {
    param([string]$PathList, [string]$Dir)
    $Want = $Dir.TrimEnd('\')
    foreach ($Entry in ($PathList -split ';')) {
        if ($Entry.Trim().TrimEnd('\') -ieq $Want) { return $true }
    }
    return $false
}

# Persist in the user's PATH (Windows registry; skipped on other platforms).
if ($IsWindows -or (-not (Test-Path variable:IsWindows))) {
    $UserPath = [System.Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not (Test-PathEntry $UserPath $InstallDir)) {
        Write-Info 'Adding to PATH...'
        $NewPath = if ($UserPath) { "$InstallDir;$UserPath" } else { $InstallDir }
        [System.Environment]::SetEnvironmentVariable('Path', $NewPath, 'User')
    }
}

# Also update the current session so the user can run termide right away.
if (-not (Test-PathEntry $env:Path $InstallDir)) {
    $env:Path = "$InstallDir;$env:Path"
}

# --- done -------------------------------------------------------------------

Write-Host ''
Write-Info "Termide $Version installed successfully!"
Write-Info "Run 'termide' to start."

} catch {
    Write-Host "x $($_.Exception.Message)" -ForegroundColor Red
} finally {
    if ($TempDir) { Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue }
}

}
