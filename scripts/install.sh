#!/usr/bin/env bash
#
# LM Translate — build and install from source on macOS and Linux.
#
#   bash scripts/install.sh                 # check, build, install (prompts first)
#   bash scripts/install.sh --yes           # no prompts
#   bash scripts/install.sh --dry-run       # print every action, change nothing
#   bash scripts/install.sh --skip-build    # install an already-built bundle
#
# Nothing privileged happens without either a prompt or --yes, and every sudo command is
# printed before it runs.

set -euo pipefail

ASSUME_YES=0
DRY_RUN=0
SKIP_BUILD=0

for arg in "$@"; do
  case "$arg" in
    -y|--yes) ASSUME_YES=1 ;;
    -n|--dry-run) DRY_RUN=1 ;;
    -s|--skip-build) SKIP_BUILD=1 ;;
    -h|--help)
      sed -n '2,11p' "$0" | sed 's/^#\s\?//'
      exit 0
      ;;
    *)
      printf 'Unknown option: %s (try --help)\n' "$arg" >&2
      exit 2
      ;;
  esac
done

# ── Output helpers ─────────────────────────────────────────────────────────

if [ -t 1 ]; then
  BOLD=$(printf '\033[1m'); DIM=$(printf '\033[2m')
  BLUE=$(printf '\033[34m'); AMBER=$(printf '\033[33m')
  RED=$(printf '\033[31m'); RESET=$(printf '\033[0m')
else
  BOLD=; DIM=; BLUE=; AMBER=; RED=; RESET=
fi

step() { printf '\n%s==>%s %s%s%s\n' "$BLUE" "$RESET" "$BOLD" "$1" "$RESET"; }
info() { printf '    %s\n' "$1"; }
ok()   { printf '    %s✓%s %s\n' "$BLUE" "$RESET" "$1"; }
warn() { printf '    %s!%s %s\n' "$AMBER" "$RESET" "$1"; }
die()  { printf '\n%serror:%s %s\n' "$RED" "$RESET" "$1" >&2; exit 1; }

# A blocker. Under --dry-run it is reported and stepped over, so the whole plan can be
# previewed on a machine that is not set up yet; a real run stops.
missing() {
  if [ "$DRY_RUN" -eq 1 ]; then
    warn "$1"
    warn "(a real run would stop here)"
  else
    die "$1"
  fi
}

run() {
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '    %swould run:%s %s\n' "$DIM" "$RESET" "$*"
  else
    "$@"
  fi
}

confirm() {
  if [ "$ASSUME_YES" -eq 1 ] || [ "$DRY_RUN" -eq 1 ]; then
    return 0
  fi
  printf '    %s [y/N] ' "$1"
  read -r reply </dev/tty || reply=n
  case "$reply" in [yY]*) return 0 ;; *) return 1 ;; esac
}

# True when $1 >= $2, comparing major.minor only.
version_ge() {
  have_major=${1%%.*}
  have_rest=${1#*.}
  have_minor=${have_rest%%.*}
  want_major=${2%%.*}
  want_rest=${2#*.}
  want_minor=${want_rest%%.*}

  if [ "$have_major" -gt "$want_major" ]; then return 0; fi
  if [ "$have_major" -lt "$want_major" ]; then return 1; fi
  if [ "$have_minor" -ge "$want_minor" ]; then return 0; fi
  return 1
}

cd "$(dirname "$0")/.."
PROJECT_ROOT=$(pwd)

NODE_MIN=20.0
RUST_MIN=1.85

printf '%sLM Translate installer%s  %s(%s)%s\n' "$BOLD" "$RESET" "$DIM" "$PROJECT_ROOT" "$RESET"
if [ "$DRY_RUN" -eq 1 ]; then
  warn "dry run: nothing will be installed or modified"
fi

# ── Platform ───────────────────────────────────────────────────────────────

step "Identifying the system"

OS=$(uname -s)
PM=none

case "$OS" in
  Darwin)
    PLATFORM=macos
    info "macOS $(sw_vers -productVersion 2>/dev/null || echo '')"
    ;;
  Linux)
    PLATFORM=linux
    if [ -r /etc/os-release ]; then
      # shellcheck disable=SC1091
      . /etc/os-release
      info "${PRETTY_NAME:-Linux}"
    fi
    if command -v apt-get >/dev/null 2>&1; then PM=apt
    elif command -v dnf >/dev/null 2>&1; then PM=dnf
    elif command -v pacman >/dev/null 2>&1; then PM=pacman
    elif command -v zypper >/dev/null 2>&1; then PM=zypper
    fi
    info "package manager: $PM"
    if [ "${XDG_SESSION_TYPE:-}" = "wayland" ]; then
      warn "Wayland session: screen capture goes through xdg-desktop-portal, and"
      warn "select-and-translate needs the primary-selection protocol. See the README."
    fi
    ;;
  *)
    die "unsupported system '$OS'. This script handles macOS and Linux; use scripts/install.ps1 on Windows."
    ;;
esac

# ── Toolchain ──────────────────────────────────────────────────────────────

step "Checking the build toolchain"

if command -v node >/dev/null 2>&1; then
  NODE_VERSION=$(node -v | sed 's/^v//')
  if version_ge "$NODE_VERSION" "$NODE_MIN"; then
    ok "node $NODE_VERSION"
  else
    missing "node $NODE_VERSION is too old; $NODE_MIN or newer is required. See https://nodejs.org"
  fi
else
  missing "node is not installed. Install Node $NODE_MIN or newer from https://nodejs.org (or your package manager)."
fi

if ! command -v npm >/dev/null 2>&1; then
  missing "npm is not installed; it normally ships with Node."
fi

if command -v cargo >/dev/null 2>&1; then
  RUST_VERSION=$(rustc --version | awk '{print $2}')
  if version_ge "$RUST_VERSION" "$RUST_MIN"; then
    ok "rust $RUST_VERSION"
  else
    warn "rust $RUST_VERSION is older than $RUST_MIN"
    if confirm "Run 'rustup update stable'?"; then
      run rustup update stable
    else
      missing "rust $RUST_MIN or newer is required."
    fi
  fi
else
  warn "rust is not installed"
  if confirm "Install it with rustup (from https://sh.rustup.rs)?"; then
    if [ "$DRY_RUN" -eq 1 ]; then
      info "would run: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
    else
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
      # shellcheck disable=SC1091
      . "$HOME/.cargo/env"
    fi
  else
    missing "rust is required. See https://rustup.rs"
  fi
fi

# ── System dependencies ────────────────────────────────────────────────────

step "Checking system dependencies"

if [ "$PLATFORM" = macos ]; then
  if xcode-select -p >/dev/null 2>&1; then
    ok "Xcode command line tools"
  else
    warn "Xcode command line tools are missing"
    if confirm "Run 'xcode-select --install'?"; then
      run xcode-select --install
      info "Re-run this script once the installer finishes."
      exit 0
    else
      die "the command line tools are required to compile."
    fi
  fi
else
  case "$PM" in
    apt)
      DEPS="libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev build-essential pkg-config curl file"
      INSTALL="apt-get install -y"
      ;;
    dnf)
      DEPS="webkit2gtk4.1-devel libappindicator-gtk3-devel librsvg2-devel libxdo-devel openssl-devel curl file"
      INSTALL="dnf install -y"
      ;;
    pacman)
      DEPS="webkit2gtk-4.1 libappindicator-gtk3 librsvg xdotool openssl base-devel curl file"
      INSTALL="pacman -S --needed --noconfirm"
      ;;
    zypper)
      DEPS="webkit2gtk3-devel libappindicator3-devel librsvg-devel xdotool openssl-devel curl file"
      INSTALL="zypper install -y"
      ;;
    *)
      warn "no supported package manager found; install the equivalents of"
      warn "webkit2gtk-4.1, libayatana-appindicator3, librsvg2, libxdo and libssl yourself."
      DEPS=""
      INSTALL=""
      ;;
  esac

  if [ -n "$DEPS" ]; then
    info "needs: $DEPS"
    if confirm "Install these with sudo $INSTALL?"; then
      # shellcheck disable=SC2086
      run sudo $INSTALL $DEPS
    else
      warn "skipping — the build will fail if any are missing"
    fi
  fi
fi

# ── Build ──────────────────────────────────────────────────────────────────

# Pick one bundle format rather than "all": AppImage packaging pulls extra tooling over
# the network and its failure would take an otherwise good build down with it.
if [ "$PLATFORM" = macos ]; then
  BUNDLES="app,dmg"
else
  case "$PM" in
    apt) BUNDLES=deb ;;
    dnf|zypper) BUNDLES=rpm ;;
    *) BUNDLES=appimage ;;
  esac
fi

if [ "$SKIP_BUILD" -eq 1 ]; then
  step "Skipping build (--skip-build)"
else
  step "Installing npm dependencies"
  if [ -f package-lock.json ]; then
    run npm ci
  else
    run npm install
  fi

  step "Building the release bundle ($BUNDLES)"
  info "this compiles the Rust core and takes a few minutes the first time"
  run npx tauri build --bundles "$BUNDLES"
fi

# ── Install ────────────────────────────────────────────────────────────────

step "Installing"

BUNDLE_DIR="$PROJECT_ROOT/src-tauri/target/release/bundle"

find_one() {
  # $1 = subdirectory, $2 = glob
  if [ -d "$BUNDLE_DIR/$1" ]; then
    find "$BUNDLE_DIR/$1" -maxdepth 1 -name "$2" -print 2>/dev/null | head -n1
  fi
}

if [ "$PLATFORM" = macos ]; then
  APP=$(find_one macos '*.app')
  if [ -z "$APP" ] && [ "$DRY_RUN" -eq 0 ]; then
    die "no .app bundle in $BUNDLE_DIR/macos — did the build succeed?"
  fi
  TARGET_DIR=/Applications
  if [ ! -w "$TARGET_DIR" ] && [ "$DRY_RUN" -eq 0 ]; then
    info "/Applications is not writable, so this needs sudo"
    run sudo rm -rf "$TARGET_DIR/LM Translate.app"
    run sudo cp -R "$APP" "$TARGET_DIR/"
  else
    run rm -rf "$TARGET_DIR/LM Translate.app"
    run cp -R "${APP:-<app bundle>}" "$TARGET_DIR/"
  fi
  ok "installed to $TARGET_DIR"
else
  case "$BUNDLES" in
    deb)
      PKG=$(find_one deb '*.deb')
      if [ -z "$PKG" ] && [ "$DRY_RUN" -eq 0 ]; then
        die "no .deb in $BUNDLE_DIR/deb — did the build succeed?"
      fi
      # apt resolves runtime dependencies; dpkg alone would not.
      run sudo apt-get install -y "${PKG:-<package>.deb}"
      ;;
    rpm)
      PKG=$(find_one rpm '*.rpm')
      if [ -z "$PKG" ] && [ "$DRY_RUN" -eq 0 ]; then
        die "no .rpm in $BUNDLE_DIR/rpm — did the build succeed?"
      fi
      if [ "$PM" = dnf ]; then
        run sudo dnf install -y "${PKG:-<package>.rpm}"
      else
        run sudo zypper install -y "${PKG:-<package>.rpm}"
      fi
      ;;
    appimage)
      IMG=$(find_one appimage '*.AppImage')
      if [ -z "$IMG" ] && [ "$DRY_RUN" -eq 0 ]; then
        die "no .AppImage in $BUNDLE_DIR/appimage — did the build succeed?"
      fi
      BIN_DIR="$HOME/.local/bin"
      DESKTOP_DIR="$HOME/.local/share/applications"
      ICON_DIR="$HOME/.local/share/icons/hicolor/256x256/apps"
      run mkdir -p "$BIN_DIR" "$DESKTOP_DIR" "$ICON_DIR"
      run cp "${IMG:-<app>.AppImage}" "$BIN_DIR/lmtranslate"
      run chmod +x "$BIN_DIR/lmtranslate"
      run cp "$PROJECT_ROOT/src-tauri/icons/128x128@2x.png" "$ICON_DIR/lmtranslate.png"

      if [ "$DRY_RUN" -eq 1 ]; then
        info "would write: $DESKTOP_DIR/lmtranslate.desktop"
      else
        cat > "$DESKTOP_DIR/lmtranslate.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=LM Translate
Comment=Translate anything on your screen with one keypress
Exec=$BIN_DIR/lmtranslate
Icon=lmtranslate
Terminal=false
Categories=Utility;Office;
StartupNotify=false
DESKTOP
      fi
      ok "installed to $BIN_DIR/lmtranslate"
      case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) warn "$BIN_DIR is not on your PATH" ;;
      esac
      ;;
  esac
fi

# ── Next steps ─────────────────────────────────────────────────────────────

step "Done"
info "LM Translate runs in the system tray — it has no main window."
info ""
info "1. Launch it, then open Settings from the tray icon."
info "2. Paste your API key under Provider and press \"Test connection\"."
info "3. Select text anywhere and press Alt+D, or press Alt+S to crop the screen."

if [ "$PLATFORM" = macos ]; then
  info ""
  warn "macOS needs two permissions before the hotkeys work:"
  warn "  System Settings > Privacy & Security > Accessibility    (to send Cmd+C)"
  warn "  System Settings > Privacy & Security > Screen Recording (to capture)"
fi

printf '\n'
