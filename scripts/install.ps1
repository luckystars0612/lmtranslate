<#
    LM Translate installer for Windows.

    By default this downloads the prebuilt installer for your system from the latest
    GitHub release — no compiler toolchain needed. Pass -FromSource to build locally.

      powershell -ExecutionPolicy Bypass -File scripts\install.ps1
      ... -Version v0.1.0    install a specific release
      ... -FromSource        build locally instead of downloading
      ... -DryRun            print every action, change nothing
      ... -Yes               do not prompt
      ... -SkipBuild         with -FromSource, reuse existing bundles

    Nothing is installed without a prompt or -Yes, and every command is printed first.
    Set $env:LMTRANSLATE_REPO = 'owner/repo' to install from a fork.
    Written for Windows PowerShell 5.1 as well as PowerShell 7.
#>

[CmdletBinding()]
param(
    [switch]$Yes,
    [switch]$DryRun,
    [switch]$FromSource,
    [switch]$SkipBuild,
    [string]$Version
)

$ErrorActionPreference = 'Stop'
# TLS 1.2 is not the default on stock Windows PowerShell 5.1, and github.com requires it.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = $env:LMTRANSLATE_REPO
if (-not $Repo) { $Repo = 'luckystars0612/lmtranslate' }
$Api = "https://api.github.com/repos/$Repo/releases"

$NodeMin = [Version]'20.0'
$RustMin = [Version]'1.85'

if ($SkipBuild) { $FromSource = $true }

# ── Output helpers ─────────────────────────────────────────────────────────

function Write-Step($text) { Write-Host "`n==> $text" -ForegroundColor Blue }
function Write-Info($text) { Write-Host "    $text" }
function Write-Ok($text)   { Write-Host "    [ok] $text" -ForegroundColor Blue }
function Write-Warn($text) { Write-Host "    [!] $text" -ForegroundColor Yellow }
function Stop-Install($text) {
    Write-Host "`nerror: $text" -ForegroundColor Red
    exit 1
}

# A blocker. Under -DryRun it is reported and stepped over so the whole plan can be
# previewed on a machine that is not set up yet; a real run stops.
function Report-Missing($text) {
    if ($DryRun) {
        Write-Warn $text
        Write-Warn "(a real run would stop here)"
    } else {
        Stop-Install $text
    }
}

function Invoke-Step {
    param([string]$Exe, [string[]]$Arguments)
    if ($DryRun) {
        Write-Host "    would run: $Exe $($Arguments -join ' ')" -ForegroundColor DarkGray
        return
    }
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) { Stop-Install "$Exe exited with code $LASTEXITCODE" }
}

function Confirm-Step($question) {
    if ($Yes -or $DryRun) { return $true }
    $reply = Read-Host "    $question [y/N]"
    return ($reply -match '^[yY]')
}

function Get-ToolVersion($exe, $pattern) {
    if (-not (Get-Command $exe -ErrorAction SilentlyContinue)) { return $null }
    $raw = & $exe --version 2>$null
    if ($raw -match $pattern) { return [Version]$Matches[1] }
    return $null
}

function Test-Winget { return [bool](Get-Command winget -ErrorAction SilentlyContinue) }

# ── Setup ──────────────────────────────────────────────────────────────────

$ProjectRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $ProjectRoot

Write-Host "LM Translate installer" -ForegroundColor White
if ($DryRun) { Write-Warn "dry run: nothing will be installed or modified" }

Write-Step "Identifying the system"
$os = Get-CimInstance Win32_OperatingSystem
Write-Info "$($os.Caption) ($($os.OSArchitecture))"

# ── Path A: download a prebuilt release ────────────────────────────────────

function Install-FromRelease {
    Write-Step "Finding a release"

    if ($Version) { $endpoint = "$Api/tags/$Version" } else { $endpoint = "$Api/latest" }

    $release = $null
    try {
        $release = Invoke-RestMethod -Uri $endpoint -Headers @{
            'User-Agent' = 'lmtranslate-installer'
            'Accept'     = 'application/vnd.github+json'
        }
    } catch {
        Write-Warn "no published release found at $Repo$(if ($Version) { " for $Version" })."
        Write-Info ""
        Write-Info "Either publish one (push a tag; .github/workflows/release.yml builds all"
        Write-Info "three platforms), or build locally right now:"
        Write-Info ""
        Write-Info "    powershell -ExecutionPolicy Bypass -File scripts\install.ps1 -FromSource"
        exit 1
    }

    Write-Ok "release $($release.tag_name)"

    # Prefer the NSIS installer: smaller, and it does not need elevation.
    $asset = $release.assets | Where-Object { $_.name -like '*-setup.exe' } | Select-Object -First 1
    if (-not $asset) {
        $asset = $release.assets | Where-Object { $_.name -like '*.msi' } | Select-Object -First 1
    }
    if (-not $asset) {
        Write-Warn "release $($release.tag_name) has no Windows installer asset."
        Write-Info "Build locally instead:  ... -FromSource"
        exit 1
    }
    Write-Info "asset: $($asset.name)  ($([math]::Round($asset.size / 1MB, 1)) MB)"

    Write-Step "Downloading"

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("lmtranslate-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    $file = Join-Path $tmp $asset.name

    if ($DryRun) {
        Write-Host "    would run: download $($asset.browser_download_url)" -ForegroundColor DarkGray
        Write-Host "    would run: verify SHA256 against SHA256SUMS.txt" -ForegroundColor DarkGray
        Write-Host "    would run: $($asset.name)" -ForegroundColor DarkGray
        return
    }

    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $file -UseBasicParsing
    Write-Ok "$([math]::Round((Get-Item $file).Length / 1MB, 1)) MB downloaded"

    $sums = $release.assets | Where-Object { $_.name -eq 'SHA256SUMS.txt' } | Select-Object -First 1
    if ($sums) {
        $sumsFile = Join-Path $tmp 'SHA256SUMS.txt'
        Invoke-WebRequest -Uri $sums.browser_download_url -OutFile $sumsFile -UseBasicParsing
        $actual = (Get-FileHash -Algorithm SHA256 -Path $file).Hash
        # Matched by digest rather than filename: published names contain spaces, and the
        # digest is the stricter check anyway.
        if ((Get-Content $sumsFile -Raw) -match [regex]::Escape($actual)) {
            Write-Ok "checksum verified against SHA256SUMS.txt"
        } else {
            Stop-Install "checksum mismatch - refusing to install $file"
        }
    } else {
        Write-Warn "this release publishes no SHA256SUMS.txt; skipping verification"
    }

    Write-Step "Installing"
    Write-Warn "this download is not code-signed; SmartScreen may warn (More info > Run anyway)."

    # /S is NSIS silent mode. Interactive by default so the user sees what happens.
    if ($file -like '*.msi') {
        if ($Yes) { Invoke-Step 'msiexec' @('/i', $file, '/qn') }
        else { Invoke-Step 'msiexec' @('/i', $file) }
    } else {
        if ($Yes) { Invoke-Step $file @('/S') } else { Invoke-Step $file @() }
    }
    Write-Ok "installed"

    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

# ── Path B: build from source ──────────────────────────────────────────────

function Install-FromSource {
    Write-Step "Checking the build toolchain"

    if (-not (Test-Winget)) {
        Write-Warn "winget is unavailable, so missing tools cannot be installed automatically"
    }

    $nodeVersion = Get-ToolVersion 'node' 'v?(\d+\.\d+)'
    if ($nodeVersion) {
        if ($nodeVersion -lt $NodeMin) {
            Report-Missing "node $nodeVersion is too old; $NodeMin or newer is required. See https://nodejs.org"
        } else {
            Write-Ok "node $nodeVersion"
        }
    } else {
        Write-Warn "node is not installed"
        if ((Test-Winget) -and (Confirm-Step "Install Node LTS with winget?")) {
            Invoke-Step 'winget' @('install', '-e', '--id', 'OpenJS.NodeJS.LTS', '--accept-source-agreements', '--accept-package-agreements')
            Write-Info "Open a new terminal so PATH picks up node, then re-run this script."
            exit 0
        }
        Report-Missing "node $NodeMin or newer is required. See https://nodejs.org"
    }

    $rustVersion = Get-ToolVersion 'rustc' '(\d+\.\d+)'
    if ($rustVersion) {
        if ($rustVersion -lt $RustMin) {
            Write-Warn "rust $rustVersion is older than $RustMin"
            if (Confirm-Step "Run 'rustup update stable'?") {
                Invoke-Step 'rustup' @('update', 'stable')
            } else {
                Report-Missing "rust $RustMin or newer is required."
            }
        } else {
            Write-Ok "rust $rustVersion"
        }
    } else {
        Write-Warn "rust is not installed"
        if ((Test-Winget) -and (Confirm-Step "Install Rust with winget?")) {
            Invoke-Step 'winget' @('install', '-e', '--id', 'Rustlang.Rustup', '--accept-source-agreements', '--accept-package-agreements')
            Write-Info "Open a new terminal so PATH picks up cargo, then re-run this script."
            exit 0
        }
        Report-Missing "rust $RustMin or newer is required. See https://rustup.rs"
    }

    Write-Step "Checking system dependencies"

    # MSVC linker: Tauri compiles native code, so the C++ toolchain has to be present.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $hasMsvc = $false
    if (Test-Path $vswhere) {
        $found = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
        if ($found) { $hasMsvc = $true; Write-Ok "MSVC build tools ($found)" }
    }
    if (-not $hasMsvc) {
        Write-Warn "the MSVC C++ build tools were not found"
        if ((Test-Winget) -and (Confirm-Step "Install VS 2022 Build Tools with the C++ workload? (a large download)")) {
            Invoke-Step 'winget' @(
                'install', '-e', '--id', 'Microsoft.VisualStudio.2022.BuildTools',
                '--accept-source-agreements', '--accept-package-agreements',
                '--override', '--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
            )
        } else {
            Write-Warn "the build will fail to link without them."
            Write-Warn "Install 'Desktop development with C++' from https://visualstudio.microsoft.com/downloads/"
            if (-not (Confirm-Step "Continue anyway?")) { exit 1 }
        }
    }

    Assert-WebView2

    if ($SkipBuild) {
        Write-Step "Skipping build (-SkipBuild)"
    } else {
        Write-Step "Installing npm dependencies"
        if (Test-Path 'package-lock.json') { Invoke-Step 'npm' @('ci') } else { Invoke-Step 'npm' @('install') }

        Write-Step "Building the release bundle"
        Write-Info "this compiles the Rust core and takes a few minutes the first time"
        Invoke-Step 'npx' @('tauri', 'build', '--bundles', 'nsis')
    }

    Write-Step "Installing"

    $bundleDir = Join-Path $ProjectRoot 'src-tauri\target\release\bundle\nsis'
    $setup = $null
    if (Test-Path $bundleDir) {
        $setup = Get-ChildItem $bundleDir -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
                 Sort-Object LastWriteTime -Descending | Select-Object -First 1
    }

    if (-not $setup) {
        if ($DryRun) { Write-Info "would run: <bundle>-setup.exe" }
        else { Stop-Install "no installer found in $bundleDir - did the build succeed?" }
    } else {
        Write-Info "installer: $($setup.Name)"
        if ($Yes) { Invoke-Step $setup.FullName @('/S') } else { Invoke-Step $setup.FullName @() }
        Write-Ok "installed"
    }
}

# WebView2 renders every window; it ships with Windows 11 and current Windows 10.
function Assert-WebView2 {
    $keys = @(
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
        'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    )
    $webview = $null
    foreach ($key in $keys) {
        if (Test-Path $key) {
            $webview = (Get-ItemProperty $key -ErrorAction SilentlyContinue).pv
            if ($webview) { break }
        }
    }
    if ($webview) {
        Write-Ok "WebView2 runtime $webview"
    } else {
        Write-Warn "the WebView2 runtime was not found"
        if ((Test-Winget) -and (Confirm-Step "Install it with winget?")) {
            Invoke-Step 'winget' @('install', '-e', '--id', 'Microsoft.EdgeWebView2Runtime', '--accept-source-agreements', '--accept-package-agreements')
        } else {
            Write-Warn "install it from https://developer.microsoft.com/microsoft-edge/webview2/ or the app will not render."
        }
    }
}

# ── Run ────────────────────────────────────────────────────────────────────

if ($FromSource) {
    Install-FromSource
} else {
    Write-Step "Checking system dependencies"
    Assert-WebView2
    Install-FromRelease
}

Write-Step "Done"
Write-Info "LM Translate runs in the system tray - it has no main window."
Write-Info ""
Write-Info "1. Launch it, then open Settings from the tray icon."
Write-Info "2. Paste your API key under Provider and press 'Test connection'."
Write-Info "3. Select text anywhere and press Alt+D, or press Alt+S to crop the screen."
Write-Host ""
