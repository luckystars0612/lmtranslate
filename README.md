# LM Translate

A background desktop app that translates whatever you are looking at, regardless of
which application is in the foreground.

Two hotkeys, one popup:

| Hotkey | Flow |
|---|---|
| `Alt+D` | **Select and translate.** Highlight text anywhere, press the key, read the translation. |
| `Alt+S` | **Screenshot and translate.** Press the key, drag a box over anything on screen — including text in images and video — and read the translation. |

There is no icon to click and no confirmation step in either flow: the hotkey *is* the
trigger, and releasing the drag *is* the trigger.

Built with Tauri v2 (Rust core, webview UI). See
[translate-app-implementation-plan.md](translate-app-implementation-plan.md) for the
specification this implements and [DESIGN.md](DESIGN.md) for the UI design plan.

---

## Requirements

| | |
|---|---|
| **All platforms** | [Rust](https://rustup.rs) 1.85+, [Node](https://nodejs.org) 20+ |
| **Windows** | Visual Studio Build Tools with the C++ workload, WebView2 runtime (preinstalled on Windows 11 and current Windows 10) |
| **macOS** | Xcode command line tools |
| **Linux** | `webkit2gtk-4.1`, `libayatana-appindicator3`, `librsvg2`, `libxdo` (for `enigo`), `libssl-dev`, and `xdg-desktop-portal` + a compositor backend if you are on Wayland |

On Debian/Ubuntu:

```sh
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
                 libxdo-dev libssl-dev build-essential curl file
```

## Build and run

```sh
npm install
npm run tauri dev        # development, with hot reload
npm run tauri build      # release binary + installers in src-tauri/target/release/
```

The app has no main window. It starts in the system tray; the tray menu has
**Translate selection**, **Translate screen region**, **Settings** and **Quit**.

Two command line flags are available:

```sh
lmtranslate --settings    # start with the Settings window open (useful for a shortcut)
lmtranslate --self-test   # cycle the popup through all three states at every screen
                          # edge, then probe both capture backends, logging results
```

Run `--self-test` against a **release** build. Its selection probe sends a real
`Ctrl+C`, and a debug build has a console window attached — if that console happens to
hold focus at that moment, it takes the keystroke as an interrupt and exits. Release
builds have no console, so this cannot happen (and it never affects normal use, where
the keystroke goes to whichever application you selected text in).

## First run

Open **Settings** from the tray and fill in the Provider section:

- **Provider** — MiniMax, or any OpenAI-compatible endpoint.
- **Base URL** — defaults to `https://api.minimax.io/v1`.
- **Model name** — defaults to `MiniMax-M3`. Editable, so a model bump needs no rebuild.
- **API key** — stored locally, never logged, only ever sent to the Base URL above.
  **Test connection** does a cheap round trip and reports pass/fail inline.

Everything else (target language, both hotkeys, launch-on-login, popup auto-close,
theme) is optional and persists across restarts.

### Where things live

| | Windows | macOS | Linux |
|---|---|---|---|
| Settings | `%APPDATA%\vn.vinsoc.lmtranslate\settings.json` | `~/Library/Application Support/vn.vinsoc.lmtranslate/` | `~/.config/vn.vinsoc.lmtranslate/` |
| Debug log | `%LOCALAPPDATA%\vn.vinsoc.lmtranslate\logs\` | `~/Library/Logs/vn.vinsoc.lmtranslate/` | `~/.local/share/vn.vinsoc.lmtranslate/logs/` |

Both paths are shown at the bottom of **Settings → Advanced**, where the log can also
be viewed and cleared. The log holds request/response *metadata* only — never the API
key, never the text being translated.

---

## Platform behaviour

The two capture operations genuinely differ per platform, so each has its own backend
behind one trait (`platform::PlatformCapture`). **Settings → Advanced** always shows
which backend is active and what it can do on the current session.

### Windows and macOS

Selection is read by saving the clipboard, sending a synthetic `Ctrl+C` / `Cmd+C`,
reading the result and **restoring the previous clipboard contents**. Capture is direct.

On macOS the first run needs two permissions in **System Settings → Privacy &
Security**: *Accessibility* (to send the copy chord) and *Screen Recording* (to
capture). Settings → Advanced has a button that opens the right pane.

### Linux / X11

Highlighted text is read straight from the **PRIMARY selection**, so nothing is
simulated, no timing delay is absorbed and your clipboard is never touched. If an
application does not export PRIMARY, it falls back to the simulate-copy path above.
Capture is direct via XCB.

### Linux / Wayland

Wayland sandboxes both input injection and screen capture, so this is a separate
implementation rather than a variant:

- **Selection** uses the Wayland **primary-selection protocol**
  (`zwp_primary_selection_v1`), which GNOME and KDE both implement and which needs no
  permission prompt. There is no synthetic `Ctrl+C` — it is not possible. On a
  compositor without that protocol the popup says so plainly instead of hanging or
  failing silently.
- **Capture** goes through the `xdg-desktop-portal` Screenshot portal, so the
  compositor shows its own consent dialog.
- Your clipboard is never modified on Wayland.

X11 vs Wayland is detected at startup from `XDG_SESSION_TYPE`, falling back to
`WAYLAND_DISPLAY`.

---

## Project layout

```
src/                          frontend — three tiny documents, no framework
  popup.{html,ts}             the only result window (loading / result / error)
  crop-overlay.{html,ts}      fullscreen drag-to-select, one per monitor
  settings.{html,ts}          single-column grouped settings
  styles/tokens.css           the design tokens + the gloss rail

src-tauri/src/
  main.rs                     entrypoint, tray icon, plugins
  hotkeys.rs                  registration, dispatch, conflict probing
  flows.rs                    end-to-end orchestration for both triggers
  commands.rs                 everything the webviews can call
  config.rs  state.rs  logger.rs  demo.rs
  platform/                   one backend per platform behind PlatformCapture
    mod.rs common.rs windows.rs macos.rs linux_x11.rs linux_wayland.rs
  capture/                    text_selection.rs, screenshot.rs
  translate/                  provider.rs (trait), client.rs (MiniMax), prompts.rs, types.rs
  windows/                    popup_window.rs, crop_overlay.rs, settings_window.rs
```

`scripts/make-icon.mjs` regenerates `app-icon.png` (a hand-rolled PNG encoder, no image
dependency); `npx tauri icon app-icon.png` turns it into the full icon set.

## Tests

```sh
cd src-tauri && cargo test     # model-reply parsing: clean JSON, fenced JSON, JSON
                               # buried in prose, braces inside strings, image
                               # position hints, raw fallback, in-band provider errors
npm run build                  # strict TypeScript typecheck + production bundle
```
