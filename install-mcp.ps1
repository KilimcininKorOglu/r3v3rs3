# Installs the newest r3v3rs3-mcp binary from the GitHub releases of r3v3rs3.
# Runs on Windows in PowerShell. Pass a release tag with -Version v1.7.3.
# INSTALL_DIR (default %LOCALAPPDATA%\Programs\r3v3rs3-mcp) overrides the target.
param(
    [string]$Version = "",
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\r3v3rs3-mcp"
)

$ErrorActionPreference = "Stop"
$Repo = "KilimcininKorOglu/r3v3rs3"
$Headers = @{ "user-agent" = "r3v3rs3-mcp installer" }

switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { $Asset = "r3v3rs3-mcp-x86_64-pc-windows-msvc.zip" }
    default {
        Write-Error "r3v3rs3-mcp ships no archive for $env:PROCESSOR_ARCHITECTURE. Linux and macOS use install-mcp.sh."
    }
}

if (-not $Version) {
    $Latest = Invoke-RestMethod -Headers $Headers "https://api.github.com/repos/$Repo/releases/latest"
    $Version = $Latest.tag_name
}
if (-not $Version.StartsWith("v")) { $Version = "v$Version" }
$Base = "https://github.com/$Repo/releases/download/$Version"

$Work = Join-Path ([System.IO.Path]::GetTempPath()) ("r3v3rs3-mcp-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $Work | Out-Null
try {
    Write-Host "Downloading r3v3rs3-mcp $Version..."
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
    Copy-Item "$Work\unpack\r3v3rs3-mcp.exe" "$InstallDir\r3v3rs3-mcp.exe" -Force
    Write-Host "r3v3rs3-mcp $Version is installed in $InstallDir."
    if ($env:PATH -notlike "*$InstallDir*") {
        Write-Host "Add $InstallDir to your PATH to run r3v3rs3-mcp from any directory."
    }
    Write-Host "Run 'r3v3rs3-mcp update' to upgrade it later."
}
finally {
    Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
