<#
    LM Translate — build and install from source on Windows.

      powershell -ExecutionPolicy Bypass -File scripts\install.ps1
      ... -Yes          no prompts
      ... -DryRun       print every action, change nothing
      ... -SkipBuild    install an already-built bundle

    Nothing is installed without either a prompt or -Yes, and every command is printed
    before it runs. Written for Windows PowerShell 5.1 as well as PowerShell 7.
#>

[CmdletBinding()]
param(
    [switch]$Yes,
    [switch]$DryRun,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

$NodeMin = [Version]'20.0'
$RustMin = [Version]'1.85'

# ── Output helpers ─────────────────────────────────────────────────────────

function Write-Step($text) { Write-Host "`n==> $text" -ForegroundColor Blue }
function Write-Info($text) { Write-Host "    $text" }
function Write-Ok($text)   { Write-Host "    [ok] $text" -ForegroundColor Blue }
function Write-Warn($text) { Write-Host "    [!] $text" -ForegroundColor Yellow }
function Stop-Install($text) {
    Write-Host "`nerror: $text" -ForegroundColor Red
    exit 1
}

function Invoke-Step {
    param([string]$Exe, [string[]]$Arguments)
    if ($DryRun) {
        Write-Host "    would run: $Exe $($Arguments -join ' ')" -ForegroundColor DarkGray
        return
    }
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) {
        Stop-Install "$Exe exited with code $LASTEXITCODE"
    }
}

function Confirm-Step($question) {
    if ($Yes -or $DryRun) { return $true }
    $reply = Read-Host "    $question [y/N]"
    return ($reply -match '^[yY]')
}

function Get-ToolVersion($exe, $pattern) {
    $cmd = Get-Command $exe -ErrorAction SilentlyContinue
    if (-not $cmd) { return $null }
    $raw = & $exe --version 2>$null
    if ($raw -match $pattern) { return [Version]$Matches[1] }
    return $null
}

function Test-Winget {
    return [bool](Get-Command winget -ErrorAction SilentlyContinue)
}

# ── Setup ──────────────────────────────────────────────────────────────────

$ProjectRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $ProjectRoot

Write-Host "LM Translate installer" -ForegroundColor White
Write-Host "  $ProjectRoot" -ForegroundColor DarkGray
if ($DryRun) { Write-Warn "dry run: nothing will be installed or modified" }

Write-Step "Identifying the system"
$os = Get-CimInstance Win32_OperatingSystem
Write-Info "$($os.Caption) ($($os.OSArchitecture))"
if (-not (Test-Winget)) {
    Write-Warn "winget is unavailable, so missing tools cannot be installed automatically"
}

# ── Toolchain ──────────────────────────────────────────────────────────────

Write-Step "Checking the build toolchain"

$nodeVersion = Get-ToolVersion 'node' 'v?(\d+\.\d+)'
if ($nodeVersion) {
    if ($nodeVersion -lt $NodeMin) {
        Stop-Install "node $nodeVersion is too old; $NodeMin or newer is required. See https://nodejs.org"
    }
    Write-Ok "node $nodeVersion"
} else {
    Write-Warn "node is not installed"
    if ((Test-Winget) -and (Confirm-Step "Install Node LTS with winget?")) {
        Invoke-Step 'winget' @('install', '-e', '--id', 'OpenJS.NodeJS.LTS', '--accept-source-agreements', '--accept-package-agreements')
        Write-Info "Open a new terminal so PATH picks up node, then re-run this script."
        exit 0
    }
    Stop-Install "node $NodeMin or newer is required. See https://nodejs.org"
}

$rustVersion = Get-ToolVersion 'rustc' '(\d+\.\d+)'
if ($rustVersion) {
    if ($rustVersion -lt $RustMin) {
        Write-Warn "rust $rustVersion is older than $RustMin"
        if (Confirm-Step "Run 'rustup update stable'?") {
            Invoke-Step 'rustup' @('update', 'stable')
        } else {
            Stop-Install "rust $RustMin or newer is required."
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
    Stop-Install "rust $RustMin or newer is required. See https://rustup.rs"
}

# ── System dependencies ────────────────────────────────────────────────────

Write-Step "Checking system dependencies"

# MSVC linker: Tauri compiles native code, so the C++ toolchain has to be present.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$hasMsvc = $false
if (Test-Path $vswhere) {
    $found = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
    if ($found) {
        $hasMsvc = $true
        Write-Ok "MSVC build tools ($found)"
    }
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

# WebView2 renders every window; it ships with Windows 11 and current Windows 10.
$webviewKeys = @(
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
    'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
)
$webview = $null
foreach ($key in $webviewKeys) {
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

# ── Build ──────────────────────────────────────────────────────────────────

if ($SkipBuild) {
    Write-Step "Skipping build (-SkipBuild)"
} else {
    Write-Step "Installing npm dependencies"
    if (Test-Path 'package-lock.json') {
        Invoke-Step 'npm' @('ci')
    } else {
        Invoke-Step 'npm' @('install')
    }

    Write-Step "Building the release bundle"
    Write-Info "this compiles the Rust core and takes a few minutes the first time"
    Invoke-Step 'npx' @('tauri', 'build', '--bundles', 'nsis')
}

# ── Install ────────────────────────────────────────────────────────────────

Write-Step "Installing"

$bundleDir = Join-Path $ProjectRoot 'src-tauri\target\release\bundle\nsis'
$setup = $null
if (Test-Path $bundleDir) {
    $setup = Get-ChildItem $bundleDir -Filter '*-setup.exe' -ErrorAction SilentlyContinue |
             Sort-Object LastWriteTime -Descending | Select-Object -First 1
}

if (-not $setup) {
    if ($DryRun) {
        Write-Info "would run: <bundle>-setup.exe"
    } else {
        Stop-Install "no installer found in $bundleDir - did the build succeed?"
    }
} else {
    Write-Info "installer: $($setup.Name)"
    # /S is NSIS silent mode. Interactive by default so the user sees what happens.
    if ($Yes) {
        Invoke-Step $setup.FullName @('/S')
    } else {
        Invoke-Step $setup.FullName @()
    }
    Write-Ok "installed"
}

# ── Next steps ─────────────────────────────────────────────────────────────

Write-Step "Done"
Write-Info "LM Translate runs in the system tray - it has no main window."
Write-Info ""
Write-Info "1. Launch it, then open Settings from the tray icon."
Write-Info "2. Paste your API key under Provider and press 'Test connection'."
Write-Info "3. Select text anywhere and press Alt+D, or press Alt+S to crop the screen."
Write-Host ""
