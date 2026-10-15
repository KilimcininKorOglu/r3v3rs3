# Installs the newest r3v3rs3-cli binary from the GitHub releases of r3v3rs3.
# Runs on Windows in PowerShell. Pass a release tag with -Version v1.7.3.
# INSTALL_DIR (default %LOCALAPPDATA%\Programs\r3v3rs3-cli) overrides the target.
param(
    [string]$Version = "",
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\r3v3rs3-cli"
)

$ErrorActionPreference = "Stop"
$Repo = "KilimcininKorOglu/r3v3rs3"
$Headers = @{ "user-agent" = "r3v3rs3-cli installer" }

switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { $Asset = "r3v3rs3-cli-x86_64-pc-windows-msvc.zip" }
    default {
        Write-Error "r3v3rs3-cli ships no archive for $env:PROCESSOR_ARCHITECTURE. Linux and macOS use install-cli.sh."
    }
}

if (-not $Version) {
    $Latest = Invoke-RestMethod -Headers $Headers "https://api.github.com/repos/$Repo/releases/latest"
    $Version = $Latest.tag_name
}
if (-not $Version.StartsWith("v")) { $Version = "v$Version" }
$Base = "https://github.com/$Repo/releases/download/$Version"

$Work = Join-Path ([System.IO.Path]::GetTempPath()) ("r3v3rs3-cli-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $Work | Out-Null
try {
    Write-Host "Downloading r3v3rs3-cli $Version..."
    Invoke-WebRequest -Headers $Headers "$Base/SHA256SUMS" -OutFile "$Work\SHA256SUMS"
    Invoke-WebRequest -Headers $Headers "$Base/$Asset" -OutFile "$Work\$Asset"

    $Expected = ($Work | ForEach-Object { Get-Content "$_\SHA256SUMS" } | ForEach-Object {
        $Parts = $_.Trim() -split "\s+"
        if ($Parts[1].TrimStart("./") -eq $Asset) { $Parts[0] }
    })
    if (-not $Expected) { throw "$Asset is not in the checksums of the release" }
    $Actual = (Get-FileHash "$Work\$Asset" -Algorithm SHA256).Hash.ToLower()
    if ($Actual -ne $Expected) { throw "the sha256 digest of $Asset does not match the checksums of the release" }

    Expand-Archive -Path "$Work\$Asset" -DestinationPath $Work\unpack
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item "$Work\unpack\r3v3rs3-cli.exe" "$InstallDir\r3v3rs3-cli.exe" -Force
    Write-Host "r3v3rs3-cli $Version is installed in $InstallDir."
    # The user Path of the registry carries the directory across sessions; the running
    # session gets it too, so the binary works without a restart of the terminal.
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (($UserPath -split ";") -notcontains $InstallDir) {
        [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
        $env:PATH += ";$InstallDir"
        Write-Host "$InstallDir was added to your user PATH."
    }
    Write-Host "Run 'r3v3rs3-cli update' to upgrade it later."
}
finally {
    Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
