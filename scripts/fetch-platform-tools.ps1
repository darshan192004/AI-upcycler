# Downloads Android platform-tools for this host into the Tauri resource dir.
#
# The binaries are Apache-2.0 and platform-specific, so they are fetched at
# build time instead of being committed (issue #125). The app looks for
# `platform-tools/adb[.exe]` beside the executable (Windows) or in
# `Contents/Resources/platform-tools` (macOS); see core/src/adb/binary.rs.
#
# Usage:  pwsh scripts/fetch-platform-tools.ps1 [-Destination <dir>]
param(
    [string]$Destination = (Join-Path $PSScriptRoot '..\src-tauri\resources\platform-tools')
)

$ErrorActionPreference = 'Stop'

$isWindowsHost = $true
$isMacHost = $false
if ($PSVersionTable.PSVersion.Major -ge 6) {
    $isWindowsHost = $IsWindows
    $isMacHost = $IsMacOS
}

if ($isWindowsHost) {
    $url = 'https://dl.google.com/android/repository/platform-tools-latest-windows.zip'
    $adb = 'adb.exe'
} elseif ($isMacHost) {
    $url = 'https://dl.google.com/android/repository/platform-tools-latest-darwin.zip'
    $adb = 'adb'
} else {
    # Linux: the release workflow does not fetch this; `adb` comes from the
    # distro package (android-tools-adb / android-tools) on PATH.
    Write-Error 'Linux builds use the distro platform-tools package; nothing to fetch.'
    exit 1
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("platform-tools-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
try {
    $zip = Join-Path $work 'platform-tools.zip'
    Write-Host "Fetching $url"
    Invoke-WebRequest -Uri $url -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath $work -Force

    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    Copy-Item -Path (Join-Path $work 'platform-tools\*') -Destination $Destination -Recurse -Force

    $resolved = Join-Path $Destination $adb
    & $resolved version
    if ($LASTEXITCODE -ne 0) {
        Write-Error "fetched $resolved but it does not run"
        exit 1
    }
    Write-Host "platform-tools ready at $Destination"
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
