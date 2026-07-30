#!/usr/bin/env bash
#
# LM Translate installer for macOS and Linux.
#
# By default this downloads the prebuilt package for your system from the latest GitHub
# release — no compiler toolchain needed. Pass --from-source to build locally instead.

set -euo pipefail

REPO="${LMTRANSLATE_REPO:-luckystars0612/lmtranslate}"
API="https://api.github.com/repos/$REPO/releases"

NODE_MIN=20.0
RUST_MIN=1.85

ASSUME_YES=0
DRY_RUN=0
FROM_SOURCE=0
SKIP_BUILD=0
# Namespaced deliberately: /etc/os-release defines VERSION, NAME, ID and friends, and
# sourcing it would silently overwrite a plain `VERSION`.
REQUESTED_VERSION=""

usage() {
  cat <<'USAGE'
LM Translate installer for macOS and Linux.

  bash scripts/install.sh                     download and install the latest release
  bash scripts/install.sh --version v0.1.0    install a specific release
  bash scripts/install.sh --from-source       build locally instead of downloading
  bash scripts/install.sh --dry-run           print every action, change nothing
  bash scripts/install.sh --yes               do not prompt
  bash scripts/install.sh --skip-build        with --from-source, reuse existing bundles

Nothing privileged runs without a prompt or --yes, and every command is printed first.
Set LMTRANSLATE_REPO=owner/repo to install from a fork.
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    -y|--yes) ASSUME_YES=1 ;;
    -n|--dry-run) DRY_RUN=1 ;;
    --from-source) FROM_SOURCE=1 ;;
    -s|--skip-build) SKIP_BUILD=1; FROM_SOURCE=1 ;;
    --version) shift; REQUESTED_VERSION="${1:-}"; [ -n "$REQUESTED_VERSION" ] || { echo "--version needs a tag" >&2; exit 2; } ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'Unknown option: %s (try --help)\n' "$1" >&2; exit 2 ;;
  esac
  shift
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

# A blocker. Under --dry-run it is reported and stepped over so the whole plan can be
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
  if [ "$ASSUME_YES" -eq 1 ] || [ "$DRY_RUN" -eq 1 ]; then return 0; fi
  printf '    %s [y/N] ' "$1"
  read -r reply </dev/tty || reply=n
  case "$reply" in [yY]*) return 0 ;; *) return 1 ;; esac
}

# True when $1 >= $2, comparing major.minor only.
version_ge() {
  have_major=${1%%.*}; have_rest=${1#*.}; have_minor=${have_rest%%.*}
  want_major=${2%%.*}; want_rest=${2#*.}; want_minor=${want_rest%%.*}
  if [ "$have_major" -gt "$want_major" ]; then return 0; fi
  if [ "$have_major" -lt "$want_major" ]; then return 1; fi
  if [ "$have_minor" -ge "$want_minor" ]; then return 0; fi
  return 1
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

cd "$(dirname "$0")/.."
PROJECT_ROOT=$(pwd)

printf '%sLM Translate installer%s\n' "$BOLD" "$RESET"
if [ "$DRY_RUN" -eq 1 ]; then warn "dry run: nothing will be installed or modified"; fi

# ── Platform ───────────────────────────────────────────────────────────────

step "Identifying the system"

OS=$(uname -s)
ARCH=$(uname -m)
PM=none

case "$OS" in
  Darwin)
    PLATFORM=macos
    info "macOS $(sw_vers -productVersion 2>/dev/null || echo '') on $ARCH"
    ;;
  Linux)
    PLATFORM=linux
    if [ -r /etc/os-release ]; then
      # Sourced in a subshell so its NAME/VERSION/ID variables cannot leak into ours.
      # shellcheck disable=SC1091
      PRETTY=$( . /etc/os-release 2>/dev/null; printf '%s' "${PRETTY_NAME:-Linux}" )
      info "$PRETTY on $ARCH"
    fi
    if command -v apt-get >/dev/null 2>&1; then PM=apt
    elif command -v dnf >/dev/null 2>&1; then PM=dnf
    elif command -v pacman >/dev/null 2>&1; then PM=pacman
    elif command -v zypper >/dev/null 2>&1; then PM=zypper
    fi
    info "package manager: $PM"
    if [ "${XDG_SESSION_TYPE:-}" = "wayland" ]; then
      warn "Wayland session: capture goes through xdg-desktop-portal, and"
      warn "select-and-translate needs the primary-selection protocol. See the README."
    fi
    ;;
  *)
    die "unsupported system '$OS'. Use scripts/install.ps1 on Windows."
    ;;
esac

# ── Path A: download a prebuilt release ────────────────────────────────────

install_from_release() {
  step "Finding a release"

  command -v curl >/dev/null 2>&1 || die "curl is required to download a release."

  if [ -n "$REQUESTED_VERSION" ]; then
    endpoint="$API/tags/$REQUESTED_VERSION"
  else
    endpoint="$API/latest"
  fi

  if ! RELEASE_JSON=$(curl -fsSL -H 'Accept: application/vnd.github+json' "$endpoint" 2>/dev/null); then
    warn "no published release found at $REPO${REQUESTED_VERSION:+ for $REQUESTED_VERSION}."
    info ""
    info "Either publish one (push a tag; .github/workflows/release.yml builds all three"
    info "platforms), or build locally right now:"
    info ""
    info "    bash scripts/install.sh --from-source"
    exit 1
  fi

  TAG=$(printf '%s' "$RELEASE_JSON" \
        | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | head -n1 | sed 's/.*"\([^"]*\)"$/\1/')
  ok "release ${TAG:-unknown}"

  # GitHub prints one browser_download_url per asset; matching on those avoids having to
  # predict filenames, which contain spaces and arrive percent-encoded.
  asset_urls() {
    printf '%s' "$RELEASE_JSON" \
      | grep -o '"browser_download_url"[[:space:]]*:[[:space:]]*"[^"]*"' \
      | sed 's/.*"\(https[^"]*\)"$/\1/'
  }

  pick_asset() { asset_urls | grep -i -- "$1" | head -n1; }

  # Which package format suits this system.
  case "$PLATFORM" in
    macos) want='\.dmg$'; kind=dmg ;;
    linux)
      case "$PM" in
        apt) want='\.deb$'; kind=deb ;;
        dnf|zypper) want='\.rpm$'; kind=rpm ;;
        *) want='\.AppImage$'; kind=appimage ;;
      esac
      ;;
  esac

  ASSET_URL=$(pick_asset "$want" || true)
  if [ -z "$ASSET_URL" ]; then
    warn "release $TAG has no $kind asset for this system."
    if [ "$PLATFORM" = linux ] && [ "$ARCH" != "x86_64" ]; then
      warn "the published Linux builds are x86_64 only, and this machine is $ARCH."
    fi
    info "Build locally instead:  bash scripts/install.sh --from-source"
    exit 1
  fi
  info "asset: $(basename "$ASSET_URL" | sed 's/%20/ /g')"

  step "Downloading"

  TMP=$(mktemp -d)
  trap 'rm -rf "$TMP"' EXIT
  FILE="$TMP/$(basename "$ASSET_URL" | sed 's/%20/_/g')"

  run curl -fsSL --retry 3 -o "$FILE" "$ASSET_URL"

  if [ "$DRY_RUN" -eq 0 ]; then
    ok "$(du -h "$FILE" | awk '{print $1}') downloaded"

    SUMS_URL=$(pick_asset 'SHA256SUMS.txt' || true)
    if [ -n "$SUMS_URL" ] && curl -fsSL -o "$TMP/SHA256SUMS.txt" "$SUMS_URL"; then
      # Compared by hash rather than by filename: the published names contain spaces and
      # are percent-encoded in URLs, so matching the digest is both simpler and stricter
      # about the bytes actually being a published artifact.
      actual=$(sha256_of "$FILE")
      if grep -qi -- "$actual" "$TMP/SHA256SUMS.txt"; then
        ok "checksum verified against SHA256SUMS.txt"
      else
        die "checksum mismatch — refusing to install $FILE"
      fi
    else
      warn "this release publishes no SHA256SUMS.txt; skipping verification"
    fi
  fi

  step "Installing"

  case "$kind" in
    deb)
      # apt resolves runtime dependencies; dpkg alone would not.
      run sudo apt-get install -y "$FILE"
      ;;
    rpm)
      if [ "$PM" = dnf ]; then run sudo dnf install -y "$FILE"; else run sudo zypper install -y "$FILE"; fi
      ;;
    appimage)
      BIN_DIR="$HOME/.local/bin"
      DESKTOP_DIR="$HOME/.local/share/applications"
      run mkdir -p "$BIN_DIR" "$DESKTOP_DIR"
      run cp "$FILE" "$BIN_DIR/lmtranslate"
      run chmod +x "$BIN_DIR/lmtranslate"
      write_desktop_entry "$BIN_DIR/lmtranslate"
      ok "installed to $BIN_DIR/lmtranslate"
      case ":$PATH:" in *":$BIN_DIR:"*) ;; *) warn "$BIN_DIR is not on your PATH" ;; esac
      ;;
    dmg)
      install_macos_dmg "$FILE"
      ;;
  esac
}

write_desktop_entry() {
  target=$1
  entry="$HOME/.local/share/applications/lmtranslate.desktop"
  if [ "$DRY_RUN" -eq 1 ]; then
    info "would write: $entry"
    return
  fi
  cat > "$entry" <<DESKTOP
[Desktop Entry]
Type=Application
Name=LM Translate
Comment=Translate anything on your screen with one keypress
Exec=$target
Icon=lmtranslate
Terminal=false
Categories=Utility;Office;
StartupNotify=false
DESKTOP
}

install_macos_dmg() {
  dmg=$1
  if [ "$DRY_RUN" -eq 1 ]; then
    info "would run: hdiutil attach / cp -R 'LM Translate.app' /Applications / hdiutil detach"
    info "would offer: xattr -dr com.apple.quarantine '/Applications/LM Translate.app'"
    return
  fi

  mount=$(hdiutil attach -nobrowse -readonly "$dmg" | awk '/\/Volumes\//{ $1=""; $2=""; sub(/^ +/,""); print; exit }')
  [ -n "$mount" ] || die "could not mount $dmg"

  app=$(find "$mount" -maxdepth 1 -name '*.app' -print | head -n1)
  if [ -z "$app" ]; then
    hdiutil detach "$mount" >/dev/null 2>&1 || true
    die "no .app inside the disk image"
  fi

  target="/Applications/$(basename "$app")"
  if [ -w /Applications ]; then
    rm -rf "$target" && cp -R "$app" /Applications/
  else
    info "/Applications needs admin rights"
    sudo rm -rf "$target" && sudo cp -R "$app" /Applications/
  fi
  hdiutil detach "$mount" >/dev/null 2>&1 || true
  ok "installed to $target"

  # These builds are unsigned, so Gatekeeper quarantines anything downloaded.
  warn "this download is not code-signed, so macOS will refuse to open it once."
  if confirm "Remove the quarantine flag now (equivalent to right-click > Open)?"; then
    run sudo xattr -dr com.apple.quarantine "$target"
  else
    info "Right-click the app in Finder and choose Open the first time."
  fi
}

# ── Path B: build from source ──────────────────────────────────────────────

install_from_source() {
  step "Checking the build toolchain"

  if command -v node >/dev/null 2>&1; then
    NODE_VERSION=$(node -v | sed 's/^v//')
    if version_ge "$NODE_VERSION" "$NODE_MIN"; then
      ok "node $NODE_VERSION"
    else
      missing "node $NODE_VERSION is too old; $NODE_MIN or newer is required. See https://nodejs.org"
    fi
  else
    missing "node is not installed. Install $NODE_MIN or newer from https://nodejs.org"
  fi

  command -v npm >/dev/null 2>&1 || missing "npm is not installed; it ships with Node."

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
        missing "the command line tools are required to compile."
      fi
    fi
  else
    case "$PM" in
      apt)
        DEPS="libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev build-essential pkg-config curl file"
        INSTALL="apt-get install -y" ;;
      dnf)
        DEPS="webkit2gtk4.1-devel libappindicator-gtk3-devel librsvg2-devel libxdo-devel openssl-devel curl file"
        INSTALL="dnf install -y" ;;
      pacman)
        DEPS="webkit2gtk-4.1 libappindicator-gtk3 librsvg xdotool openssl base-devel curl file"
        INSTALL="pacman -S --needed --noconfirm" ;;
      zypper)
        DEPS="webkit2gtk3-devel libappindicator3-devel librsvg-devel xdotool openssl-devel curl file"
        INSTALL="zypper install -y" ;;
      *)
        warn "no supported package manager found; install the equivalents of"
        warn "webkit2gtk-4.1, libayatana-appindicator3, librsvg2, libxdo and libssl yourself."
        DEPS=""; INSTALL="" ;;
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

  # One bundle format rather than "all": AppImage packaging pulls extra tooling over the
  # network, and its failure would take an otherwise good build down with it.
  if [ "$PLATFORM" = macos ]; then
    BUNDLES="app,dmg"; kind=dmg
  else
    case "$PM" in
      apt) BUNDLES=deb; kind=deb ;;
      dnf|zypper) BUNDLES=rpm; kind=rpm ;;
      *) BUNDLES=appimage; kind=appimage ;;
    esac
  fi

  if [ "$SKIP_BUILD" -eq 1 ]; then
    step "Skipping build (--skip-build)"
  else
    step "Installing npm dependencies"
    if [ -f package-lock.json ]; then run npm ci; else run npm install; fi

    step "Building the release bundle ($BUNDLES)"
    info "this compiles the Rust core and takes a few minutes the first time"
    run npx tauri build --bundles "$BUNDLES"
  fi

  step "Installing"

  BUNDLE_DIR="$PROJECT_ROOT/src-tauri/target/release/bundle"
  find_one() {
    if [ -d "$BUNDLE_DIR/$1" ]; then
      find "$BUNDLE_DIR/$1" -maxdepth 1 -name "$2" -print 2>/dev/null | head -n1
    fi
  }

  case "$kind" in
    deb)
      PKG=$(find_one deb '*.deb')
      [ -n "$PKG" ] || [ "$DRY_RUN" -eq 1 ] || die "no .deb in $BUNDLE_DIR/deb"
      run sudo apt-get install -y "${PKG:-<package>.deb}" ;;
    rpm)
      PKG=$(find_one rpm '*.rpm')
      [ -n "$PKG" ] || [ "$DRY_RUN" -eq 1 ] || die "no .rpm in $BUNDLE_DIR/rpm"
      if [ "$PM" = dnf ]; then run sudo dnf install -y "${PKG:-<package>.rpm}"
      else run sudo zypper install -y "${PKG:-<package>.rpm}"; fi ;;
    appimage)
      IMG=$(find_one appimage '*.AppImage')
      [ -n "$IMG" ] || [ "$DRY_RUN" -eq 1 ] || die "no .AppImage in $BUNDLE_DIR/appimage"
      BIN_DIR="$HOME/.local/bin"
      run mkdir -p "$BIN_DIR" "$HOME/.local/share/applications"
      run cp "${IMG:-<app>.AppImage}" "$BIN_DIR/lmtranslate"
      run chmod +x "$BIN_DIR/lmtranslate"
      write_desktop_entry "$BIN_DIR/lmtranslate"
      ok "installed to $BIN_DIR/lmtranslate" ;;
    dmg)
      APP=$(find_one macos '*.app')
      if [ "$DRY_RUN" -eq 1 ]; then
        info "would run: cp -R <app bundle> /Applications/"
      else
        [ -n "$APP" ] || die "no .app in $BUNDLE_DIR/macos"
        target="/Applications/$(basename "$APP")"
        if [ -w /Applications ]; then rm -rf "$target" && cp -R "$APP" /Applications/
        else sudo rm -rf "$target" && sudo cp -R "$APP" /Applications/; fi
        ok "installed to $target"
        # A locally built app is not quarantined, so Gatekeeper stays out of the way.
      fi ;;
  esac
}

# ── Run ────────────────────────────────────────────────────────────────────

if [ "$FROM_SOURCE" -eq 1 ]; then
  install_from_source
else
  install_from_release
fi

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
