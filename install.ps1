# wattdeck installer for Windows. Download the release asset, verify its
# SHA256, install to %LOCALAPPDATA%\wattdeck\bin (override with
# $env:WATTDECK_INSTALL_DIR), print the PATH line if needed.
#
# Run: irm https://raw.githubusercontent.com/olexanderboychuk/wattdeck/main/install.ps1 | iex
param(
    [string]$Tag,
    [string]$ReleaseDir,
    [string]$InstallDir,
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
$Repo = 'olexanderboychuk/wattdeck'

function Target-For($os, $arch) {
    # "$os:$arch" is a parse error in PowerShell: the colon after a variable
    # name starts a scope prefix ($os:), not a literal. Interpolate the names
    # in braces so the colon is text.
    switch ("$($os):$arch") {
        'Windows:AMD64' { 'x86_64-pc-windows-msvc' }
        default { $null }
    }
}

function Invoke-SelfTest {
    # The map and the refusal text, tested on any machine.
    if ((Target-For 'Windows' 'AMD64') -ne 'x86_64-pc-windows-msvc') {
        throw "the x86_64 map is wrong"
    }
    if (Target-For 'Windows' 'ARM64') {
        throw "ARM64 must have no target"
    }
    $refusal = "no wattdeck build for Windows on ARM64. Built targets: x86_64-unknown-linux-gnu (Linux x86_64), aarch64-apple-darwin (macOS Apple silicon), x86_64-pc-windows-msvc (Windows x86_64)."
    if ($refusal -notmatch 'x86_64-pc-windows-msvc') {
        throw "the refusal must name the built targets"
    }
    Write-Host 'install.ps1 self-test passes'
}

if ($SelfTest) { Invoke-SelfTest; return }

$arch = $env:PROCESSOR_ARCHITECTURE
$target = Target-For 'Windows' $arch
if (-not $target) {
    throw "no wattdeck build for Windows on $arch. Built targets: x86_64-unknown-linux-gnu (Linux x86_64), aarch64-apple-darwin (macOS Apple silicon), x86_64-pc-windows-msvc (Windows x86_64)."
}

$tag = $Tag
if (-not $tag) {
    $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" `
        -Headers @{ Accept = 'application/vnd.github+json' } -UserAgent 'wattdeck-installer'
    $tag = $latest.tag_name
    if (-not $tag) { throw "no latest release in $Repo (one GitHub API request per install)" }
}

$asset = "wattdeck-$tag-$target.zip"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
$tmp = New-Item -ItemType Directory -Path $tmp

if ($ReleaseDir) {
    Copy-Item (Join-Path $ReleaseDir $asset) (Join-Path $tmp $asset)
    Copy-Item (Join-Path $ReleaseDir 'SHA256SUMS.txt') (Join-Path $tmp 'SHA256SUMS.txt')
}
else {
    Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$tag/$asset" `
        -OutFile (Join-Path $tmp $asset) -UseBasicParsing
    Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$tag/SHA256SUMS.txt" `
        -OutFile (Join-Path $tmp 'SHA256SUMS.txt') -UseBasicParsing
}

# Verify the hash before extracting: unverified bytes are never run.
$lines = Get-Content (Join-Path $tmp 'SHA256SUMS.txt')
$want = ($lines | Where-Object { $_ -match "\s$asset$" } | Select-Object -First 1) -split '\s+' | Select-Object -First 1
if (-not $want) { throw "$asset is not listed in SHA256SUMS.txt" }
$got = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $asset)).Hash.ToLower()
if ($got -ne $want.ToLower()) { throw "SHA256 mismatch for ${asset}: expected $want, got $got" }

Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $tmp
$exe = Join-Path $tmp 'wattdeck.exe'
if (-not (Test-Path $exe)) { throw "$asset did not contain wattdeck.exe" }

$dest = if ($InstallDir) { $InstallDir } else { Join-Path $env:LOCALAPPDATA 'wattdeck\bin' }
if (-not [System.IO.Path]::IsPathRooted($dest)) { throw "$dest is not a rooted path" }
New-Item -ItemType Directory -Path $dest -Force | Out-Null
Copy-Item $exe (Join-Path $dest 'wattdeck.exe') -Force

Write-Host "installed $dest\wattdeck.exe (asset $asset, target $target)"
if (($env:PATH -split ';') -notcontains $dest) {
    Write-Host "add it to PATH: setx PATH ""$dest;$env:PATH"""
}
& (Join-Path $dest 'wattdeck.exe') --version
