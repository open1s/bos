# Package BSH for Windows: a portable .zip and a per-user .msi.
#
# The MSI is built from installer/bsh.wxs with the WiX Toolset v3 that ships on
# the GitHub windows runners (or on PATH). The .zip is the no-install fallback.
# When a signing certificate is supplied, bsh.exe and the MSI are signed with
# signtool before they are written.
#
# Usage:
#   package-windows.ps1 -Version 3.0.3 -Binary target/release/bsh.exe [-Arch x64|arm64] -OutDir dist
#
# Environment:
#   WINDOWS_CERTIFICATE_FILE      path to a PFX; when set, sign bsh.exe and the MSI
#   WINDOWS_CERTIFICATE_PASSWORD  its password
#
# Outputs (in -OutDir), where <arch> is -Arch (x64 or arm64):
#   bsh-<version>-windows-<arch>.zip
#   bsh-<version>-windows-<arch>.msi

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Binary,
    # The MSI platform and the artifact name. Both runners build natively, so
    # the host architecture decides which of the two a job passes.
    [ValidateSet('x64', 'arm64')][string]$Arch = 'x64',
    [Parameter(Mandatory = $true)][string]$OutDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Fail([string]$Message) {
    Write-Error "package-windows: $Message"
    exit 1
}

if ($Version -notmatch '^(?<core>[0-9]+\.[0-9]+\.[0-9]+)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') {
    Fail "'$Version' is not a SemVer version"
}
# MSI ProductVersion is numeric only (major.minor.build, build <= 65535), so a
# prerelease suffix cannot survive into the package metadata.
$msiVersion = $Matches['core']

if (-not (Test-Path $Binary)) { Fail "binary not found: $Binary" }

$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$binaryPath = (Resolve-Path $Binary).Path
$licensePath = Join-Path $root 'LICENSE'
$readmePath = Join-Path $root 'README.md'
# Forward slashes are accepted by both Windows and PowerShell on other hosts,
# which keeps the script runnable off-Windows for a dry run.
$iconPath = Join-Path $root 'crates/bsh/src/assets/bos-harness-icon.ico'

foreach ($required in @($binaryPath, $licensePath, $readmePath, $iconPath)) {
    if (-not (Test-Path $required)) { Fail "missing input: $required" }
}

function Find-WixTool([string]$Name) {
    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }

    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} "WiX Toolset v3.14\bin"),
        (Join-Path $env:ProgramFiles "WiX Toolset v3.14\bin"),
        (Join-Path ${env:ProgramFiles(x86)} "WiX Toolset v3.11\bin")
    )
    foreach ($dir in $candidates) {
        $path = Join-Path $dir $Name
        if (Test-Path $path) { return $path }
    }
    Fail "WiX $Name not found; install WiX Toolset v3 (choco install wixtoolset -y)"
}

function Sign-File([string]$Path) {
    $certificate = $env:WINDOWS_CERTIFICATE_FILE
    if ([string]::IsNullOrEmpty($certificate)) { return }
    if (-not (Test-Path $certificate)) { Fail "certificate not found: $certificate" }
    if ([string]::IsNullOrEmpty($env:WINDOWS_CERTIFICATE_PASSWORD)) {
        Fail "WINDOWS_CERTIFICATE_PASSWORD is required when WINDOWS_CERTIFICATE_FILE is set"
    }

    $signTool = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if (-not $signTool) {
        $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
        $found = Get-ChildItem -Path $kits -Filter signtool.exe -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\' } |
            Select-Object -First 1
        if (-not $found) { Fail "signtool.exe not found" }
        $signToolPath = $found.FullName
    } else {
        $signToolPath = $signTool.Source
    }

    $signArgs = @(
        'sign', '/fd', 'SHA256', '/td', 'SHA256',
        '/tr', 'http://timestamp.digicert.com',
        '/f', $certificate,
        '/p', $env:WINDOWS_CERTIFICATE_PASSWORD,
        $Path
    )
    & $signToolPath @signArgs
    if ($LASTEXITCODE -ne 0) { Fail "signtool failed for $Path" }
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$outFull = (Resolve-Path $OutDir).Path

# --- Portable zip ----------------------------------------------------------
$portableName = "bsh-$Version-windows-$Arch"
$portableDir = Join-Path $outFull $portableName
if (Test-Path $portableDir) { Remove-Item -Recurse -Force $portableDir }
New-Item -ItemType Directory -Force -Path $portableDir | Out-Null

$stagedExe = Join-Path $portableDir 'bsh.exe'
Copy-Item $binaryPath $stagedExe
Copy-Item $licensePath (Join-Path $portableDir 'LICENSE.txt')
Copy-Item $readmePath (Join-Path $portableDir 'README.md')
Copy-Item $iconPath (Join-Path $portableDir 'bsh.ico')

Sign-File $stagedExe

$zipPath = Join-Path $outFull "$portableName.zip"
Compress-Archive -Path (Join-Path $portableDir '*') -DestinationPath $zipPath -Force

# --- MSI -------------------------------------------------------------------
$wxs = Join-Path $root 'installer/bsh.wxs'
$objDir = Join-Path $outFull 'wix'
New-Item -ItemType Directory -Force -Path $objDir | Out-Null
$wixobj = Join-Path $objDir 'bsh.wixobj'
$msiPath = Join-Path $outFull "bsh-$Version-windows-$Arch.msi"

$candle = Find-WixTool 'candle.exe'
$light = Find-WixTool 'light.exe'

$candleArgs = @(
    '-nologo', '-arch', $Arch,
    "-dPlatform=$Arch",
    "-dProductVersion=$msiVersion",
    "-dBinaryPath=$stagedExe",
    "-dLicensePath=$licensePath",
    "-dReadmePath=$readmePath",
    "-dIconPath=$iconPath",
    '-out', $wixobj, $wxs
)
& $candle @candleArgs
if ($LASTEXITCODE -ne 0) { Fail "candle failed" }

# -sval skips ICE validation: the per-user layout trips advisory ICEs (ICE61
# and friends) that are fatal by default, and this MSI has no custom actions
# for those checks to protect.
& $light -nologo -sval -out $msiPath $wixobj
if ($LASTEXITCODE -ne 0) { Fail "light failed" }

Sign-File $msiPath

foreach ($artifact in @($zipPath, $msiPath)) {
    if (-not (Test-Path $artifact)) { Fail "missing artifact: $artifact" }
    $size = (Get-Item $artifact).Length
    if ($size -lt 1MB) { Fail "$artifact is suspiciously small ($size bytes)" }
    Write-Host "package-windows: wrote $artifact ($size bytes)"
}
