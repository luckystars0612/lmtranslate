//! End-to-end orchestration for both triggers.
//!
//! `capture/` knows how to get bytes off the screen and `translate/` knows how to turn
//! them into a translation; this module is the glue that sequences them against the
//! popup, the debounce guard and the log.

use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::capture;
use crate::config::{Settings, REQUEST_TIMEOUT_SECS};
use crate::platform::CaptureError;
use crate::state::AppState;
use crate::translate::types::{InputKind, ProviderError, TranslateRequest};
use crate::windows::crop_overlay::{self, FrozenFrame};
use crate::windows::popup_window::{self, Anchor, PopupState};
use crate::windows::{selection_bubble, settings_window};

/// Slack on top of the HTTP client's own timeout, so the client reports first.
const OVERALL_TIMEOUT_SECS: u64 = REQUEST_TIMEOUT_SECS + 5;
/// Give the compositor time to actually stop drawing the overlays before capturing.
const OVERLAY_TEARDOWN_GRACE: Duration = Duration::from_millis(70);
const OVERLAY_TEARDOWN_LIMIT: Duration = Duration::from_millis(700);

// ── Flow 1: select and translate ───────────────────────────────────────────

/// What a selection hotkey should do with the translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextMode {
    /// Translate into the default target language and show it in the popup.
    Popup,
    /// Translate into the rewrite language and paste it over the selection, replacing
    /// what the user wrote.
    Replace,
}

pub fn start_text_flow(app: AppHandle, mode: TextMode) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };

    // Debounce: a second press while a request is in flight is ignored rather than
    // stacking another request behind it.
    if !state.begin_flight() {
        state
            .logger
            .info("select-and-translate ignored: a translation is already running");
        return;
    }

    let generation = state.next_generation();
    let mut settings = state.settings_snapshot();
    drop(state);

    // Resolved into the per-flow copy of the settings, so every downstream consumer —
    // the popup's route line and language dropdown, the request, the cache key — reads
    // the language this particular press asked for without any extra plumbing.
    if mode == TextMode::Replace {
        settings.target_lang = settings.replace_lang.clone();
    }

    tauri::async_runtime::spawn(text_flow(app, generation, settings, mode));
}

async fn text_flow(app: AppHandle, generation: u64, settings: Settings, mode: TextMode) {
    // The synthetic Ctrl+C has to reach the user's application rather than us.
    {
        let app = app.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            focus_selection_owner(&app);
        })
        .await;
    }

    // A rewrite the user asked to be silent shows nothing up front — but the anchor is
    // still recorded, so if it fails the popup can appear next to the cursor.
    let quiet = mode == TextMode::Replace && !settings.rewrite_show_popup;
    let anchor = popup_window::cursor_anchor(&app);
    if quiet {
        popup_window::set_anchor(&app, anchor);
    } else {
        popup_window::show(&app, anchor, PopupState::loading(&settings, "text"));
    }

    let grab = {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let state = app.state::<AppState>();
            capture::text_selection::grab_selected_text(state.platform.as_ref())
        })
        .await
    };

    let text = match grab {
        Ok(Ok(text)) => text,
        Ok(Err(error)) => {
            let (title, detail, hint, fixable) = describe_capture_error(&error, "text");
            log_warn(&app, &format!("selection capture failed: {error}"));
            let mut popup = PopupState::error(&settings, "text", title, detail, hint);
            if fixable {
                popup = popup.offering_settings();
            }
            settle(&app, generation, popup, &settings, false, quiet);
            return;
        }
        Err(join_error) => {
            log_error(&app, &format!("capture task panicked: {join_error}"));
            settle(
                &app,
                generation,
                PopupState::error(
                    &settings,
                    "text",
                    "Could not read the selection",
                    join_error.to_string(),
                    "Try again; if it keeps happening, check the debug log in Settings.",
                ),
                &settings,
                false,
                quiet,
            );
            return;
        }
    };

    // In popup mode the copy is done, so it is now safe to take focus — Esc and the copy
    // button both need it. In replace mode we must NOT: the paste that comes after the
    // translation has to land in the user's application, and focusing the popup would
    // send Ctrl+V to us instead.
    if mode == TextMode::Popup {
        popup_window::focus(&app);
    }

    if !quiet {
        popup_window::update(
            &app,
            PopupState::loading_with_original(&settings, "text", text.clone()),
        );
    }

    run_translation(
        app,
        generation,
        InputKind::Text(text),
        "text",
        settings,
        mode == TextMode::Replace,
    )
    .await;
}

/// Makes sure the application holding the selection has keyboard focus.
///
/// Usually it already does. It does not when a popup from the previous translation was
/// still focused — closing it leaves Windows to pick what gets activated next, and that
/// is not reliably the window underneath — or when the press came from a click on the
/// selection bubble. Either way the selection is still there, just not focused, and the
/// copy came back empty: "No text selected" with the text visibly highlighted.
fn focus_selection_owner(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };

    selection_bubble::hide(app);

    let foreground = state.platform.foreground_window();
    if let Some(window) = foreground {
        if let Ok(mut slot) = state.target_window.lock() {
            *slot = Some(window);
        }
    }

    // A previous popup may still hold focus.
    popup_window::close_and_settle(app);

    if foreground.is_some() {
        return;
    }
    // One of our windows had focus. Settings is the exception: text selected inside it
    // is what the user means, and focus is already in the right place.
    let in_settings = app
        .get_webview_window(settings_window::LABEL)
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    if in_settings {
        return;
    }

    let target = state.target_window.lock().ok().and_then(|slot| *slot);
    if let Some(target) = target {
        if !state.platform.activate_window(target) {
            state
                .logger
                .warn("could not give focus back to the application with the selection");
        }
    }
}

// ── Flow 2: screenshot and translate ───────────────────────────────────────

pub fn start_screenshot_flow(app: AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };

    let settings = state.settings_snapshot();
    let supports_image = state
        .provider_snapshot()
        .map(|p| p.supports_image_input())
        .unwrap_or(false);

    if !supports_image {
        let meta = settings.active_meta();
        drop(state);
        let anchor = popup_window::cursor_anchor(&app);
        popup_window::show(
            &app,
            anchor,
            PopupState::error(
                &settings,
                "image",
                "This provider cannot read images",
                format!("{} is configured as text-only.", meta.label),
                "Pick a vision-capable model in Settings -> Provider.",
            )
            .offering_settings(),
        );
        popup_window::focus(&app);
        return;
    }

    if !state.begin_overlay() {
        return;
    }
    drop(state);
    crop_overlay::begin_session();

    tauri::async_runtime::spawn_blocking(move || {
        // Nothing of ours may end up in the frozen frame, or sit above the overlay.
        selection_bubble::hide_and_settle(&app);
        popup_window::close_and_settle(&app);

        let started = Instant::now();
        let frames = crop_overlay::freeze_all(&app);
        if !frames.is_empty() {
            log_info(
                &app,
                &format!(
                    "froze {} monitor(s) in {}ms",
                    frames.len(),
                    started.elapsed().as_millis()
                ),
            );
        }
        if let Some(state) = app.try_state::<AppState>() {
            if let Ok(mut slot) = state.frozen_frames.lock() {
                *slot = frames;
            }
        }

        let handle = app.clone();
        let _ = app.run_on_main_thread(move || match crop_overlay::open_all(&handle) {
            Ok(0) | Err(_) => {
                log_error(&handle, "could not open the crop overlay on any monitor");
                cancel_crop(handle.clone());
            }
            Ok(count) => log_info(&handle, &format!("crop overlay open on {count} monitor(s)")),
        });
    });
}

pub fn cancel_crop(app: AppHandle) {
    crop_overlay::close_all(&app);
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut frames) = state.frozen_frames.lock() {
            frames.clear();
        }
        state.end_overlay();
    }
}

/// `x`/`y`/`width`/`height` arrive in the reporting overlay's own CSS pixels.
pub fn handle_crop(app: AppHandle, label: String, x: f64, y: f64, width: f64, height: f64) {
    // Read the overlay's real geometry before tearing it down. Doing the conversion
    // here rather than in JS keeps mixed-DPI multi-monitor setups correct.
    let geometry = app.get_webview_window(&label).and_then(|window| {
        let scale = window.scale_factor().ok()?;
        let origin = window.outer_position().ok()?;
        Some((origin.x as f64, origin.y as f64, scale))
    });

    let Some((origin_x, origin_y, scale)) = geometry else {
        cancel_crop(app);
        return;
    };

    let physical_x = origin_x + x * scale;
    let physical_y = origin_y + y * scale;
    let physical_w = (width * scale).round().max(0.0) as u32;
    let physical_h = (height * scale).round().max(0.0) as u32;

    crop_overlay::close_all(&app);

    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    // Every frame is dropped here; only the one the drag was made on is kept.
    let frozen = state
        .frozen_frames
        .lock()
        .ok()
        .and_then(|mut frames| frames.drain().find(|(key, _)| *key == label))
        .map(|(_, frame)| frame);
    state.end_overlay();

    if !state.begin_flight() {
        return;
    }
    let generation = state.next_generation();
    let settings = state.settings_snapshot();
    drop(state);

    tauri::async_runtime::spawn(screenshot_flow(
        app,
        generation,
        settings,
        (
            physical_x.round() as i32,
            physical_y.round() as i32,
            physical_w,
            physical_h,
        ),
        frozen,
    ));
}

async fn screenshot_flow(
    app: AppHandle,
    generation: u64,
    settings: Settings,
    region: (i32, i32, u32, u32),
    frozen: Option<FrozenFrame>,
) {
    let (x, y, width, height) = region;

    // A live capture happens *before* the popup opens. The popup would otherwise be on
    // screen while the shot is taken and could end up inside it. A frozen frame was
    // taken before anything of ours was on screen, so it has nothing to wait for.
    if frozen.is_none() {
        wait_for_overlays_to_vanish(&app).await;
    }

    let captured = {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || match frozen {
            Some(frame) => capture::screenshot::crop_frozen(&frame, x, y, width, height),
            None => {
                let state = app.state::<AppState>();
                capture::screenshot::capture_region(state.platform.as_ref(), x, y, width, height)
            }
        })
        .await
    };

    let anchor = Anchor {
        x: x as f64,
        y: (y + height as i32) as f64,
    };

    let base64_png = match captured {
        Ok(Ok(data)) => data,
        Ok(Err(error)) => {
            let (title, detail, hint, fixable) = describe_capture_error(&error, "image");
            log_warn(&app, &format!("screen capture failed: {error}"));
            let mut popup = PopupState::error(&settings, "image", title, detail, hint);
            if fixable {
                popup = popup.offering_settings();
            }
            popup_window::show(&app, anchor, popup);
            popup_window::focus(&app);
            end_flight(&app);
            return;
        }
        Err(join_error) => {
            log_error(&app, &format!("capture task panicked: {join_error}"));
            popup_window::show(
                &app,
                anchor,
                PopupState::error(
                    &settings,
                    "image",
                    "Could not capture the screen",
                    join_error.to_string(),
                    "Try again; if it keeps happening, check the debug log in Settings.",
                ),
            );
            end_flight(&app);
            return;
        }
    };

    log_info(
        &app,
        &format!("captured {width}x{height} region ({} base64 bytes)", base64_png.len()),
    );

    popup_window::show(&app, anchor, PopupState::loading(&settings, "image"));
    popup_window::focus(&app);

    run_translation(
        app,
        generation,
        InputKind::ImageBase64(base64_png),
        "image",
        settings,
        // A screen region is not an editable selection, so there is nothing to paste over.
        false,
    )
    .await;
}

async fn wait_for_overlays_to_vanish(app: &AppHandle) {
    let deadline = Instant::now() + OVERLAY_TEARDOWN_LIMIT;
    loop {
        // The overlays are hidden, not closed, so "gone" means "no longer visible".
        let still_open = app.webview_windows().iter().any(|(label, window)| {
            crop_overlay::is_overlay(label) && window.is_visible().unwrap_or(false)
        });
        if !still_open || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    tokio::time::sleep(OVERLAY_TEARDOWN_GRACE).await;
}

// ── Re-running with a different target language ────────────────────────────

pub fn retranslate(app: AppHandle, target_lang: String) -> Result<(), String> {
    let target_lang = target_lang.trim().to_string();
    if target_lang.is_empty() {
        return Err("Pick a language first.".into());
    }

    let Some(state) = app.try_state::<AppState>() else {
        return Err("The app is shutting down.".into());
    };

    let input = state
        .last_input
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .ok_or_else(|| "There is nothing to translate again.".to_string())?;

    if !state.begin_flight() {
        return Err("A translation is already running.".into());
    }

    // A per-result override, not a settings change. Now that two languages are
    // configured and each has its own hotkey, persisting the popup's dropdown would
    // silently overwrite whichever default the user did not mean to touch.
    let mut settings = state.settings_snapshot();
    settings.target_lang = target_lang;

    let generation = state.next_generation();
    drop(state);

    let source_kind = match input {
        InputKind::Text(_) => "text",
        InputKind::ImageBase64(_) => "image",
    };
    let original = match &input {
        InputKind::Text(text) => text.clone(),
        InputKind::ImageBase64(_) => popup_window::snapshot(&app).original,
    };

    popup_window::update(
        &app,
        PopupState::loading_with_original(&settings, source_kind, original),
    );

    tauri::async_runtime::spawn(run_translation(
        app,
        generation,
        input,
        source_kind,
        settings,
        // Re-running from the popup's dropdown never pastes: the user is reading, and by
        // now the original selection may not even be there.
        false,
    ));

    Ok(())
}

// ── Shared translation step ────────────────────────────────────────────────

async fn run_translation(
    app: AppHandle,
    generation: u64,
    input: InputKind,
    source_kind: &'static str,
    settings: Settings,
    replace: bool,
) {
    // A rewrite that already edited the document has nothing to report on success.
    let quiet = replace && !settings.rewrite_show_popup;
    let started = Instant::now();
    let kind = input.label();
    let magnitude = input.magnitude();

    let original_display = match &input {
        InputKind::Text(text) => text.clone(),
        InputKind::ImageBase64(_) => String::new(),
    };

    // A repeat of the same text in the same language costs nothing. Checked before the
    // provider is even consulted; image input is not cached (see translate::cache).
    if let InputKind::Text(text) = &input {
        let cached = app
            .try_state::<AppState>()
            .and_then(|state| state.cache.lock().ok()?.get(text, &settings.target_lang));

        if let Some(response) = cached {
            let translated = response.joined_translated();
            log_info(
                &app,
                &format!("translate cache hit kind=text chars={}", translated.chars().count()),
            );
            if let Some(state) = app.try_state::<AppState>() {
                if let Ok(mut slot) = state.last_input.lock() {
                    *slot = Some(input.clone());
                }
            }
            // A cache hit still has to paste, or Alt+F would silently do nothing the
            // second time you pressed it on the same text.
            let replacement = paste_over_selection(&app, replace, &translated).await;
            let mut popup =
                PopupState::result(&settings, source_kind, original_display, translated)
                    .with_note(replacement.note);
            if replacement.failed {
                popup = popup.marking_replace_failed();
            }
            settle(&app, generation, popup, &settings, true, quiet);
            return;
        }
    }

    let provider = {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        if let Ok(mut slot) = state.last_input.lock() {
            *slot = Some(input.clone());
        }
        state.provider_snapshot()
    };

    let Some(provider) = provider else {
        settle(
            &app,
            generation,
            PopupState::error(
                &settings,
                source_kind,
                "No provider configured",
                "LM Translate has no active translation provider.",
                "Pick one in Settings -> Provider.",
            )
            .offering_settings(),
            &settings,
            false,
            quiet,
        );
        return;
    };

    // Metadata only — never the API key, never the text being translated.
    log_info(
        &app,
        &format!(
            "translate start kind={kind} size={magnitude} target={} provider={}",
            settings.target_lang, settings.provider
        ),
    );

    // Kept for the cache write below, which happens after `input` has been moved.
    let cache_key = match &input {
        InputKind::Text(text) => Some(text.clone()),
        InputKind::ImageBase64(_) => None,
    };

    let request = TranslateRequest {
        input,
        target_lang: settings.target_lang.clone(),
    };

    let outcome = tokio::time::timeout(
        Duration::from_secs(OVERALL_TIMEOUT_SECS),
        provider.translate(request),
    )
    .await
    .unwrap_or_else(|_| Err(ProviderError::Timeout(OVERALL_TIMEOUT_SECS)));

    let elapsed_ms = started.elapsed().as_millis();

    match outcome {
        Ok(response) => {
            let translated = response.joined_translated();
            let original = if source_kind == "image" {
                response.joined_original()
            } else {
                original_display
            };

            log_info(
                &app,
                &format!(
                    "translate ok kind={kind} segments={} chars={} raw={}B tokens[{}] in {elapsed_ms}ms",
                    response.segments.len(),
                    translated.chars().count(),
                    response.raw_model_output.len(),
                    response.usage.as_deref().unwrap_or("not reported"),
                ),
            );

            if translated.trim().is_empty() {
                settle(
                    &app,
                    generation,
                    PopupState::error(
                        &settings,
                        source_kind,
                        "Nothing came back",
                        "The model returned an empty translation.",
                        "Try again, or pick a different model in Settings -> Provider.",
                    )
                    .offering_settings(),
                    &settings,
                    false,
                    quiet,
                );
                return;
            }

            if let Some(text) = cache_key {
                if let Some(state) = app.try_state::<AppState>() {
                    if let Ok(mut cache) = state.cache.lock() {
                        cache.put(&text, &settings.target_lang, &response);
                    }
                }
            }

            let replacement = paste_over_selection(&app, replace, &translated).await;
            let mut popup = PopupState::result(&settings, source_kind, original, translated)
                .with_note(replacement.note);
            if replacement.failed {
                popup = popup.marking_replace_failed();
            }

            settle(&app, generation, popup, &settings, true, quiet);
        }
        Err(error) => {
            log_warn(
                &app,
                &format!("translate failed kind={kind} after {elapsed_ms}ms: {error}"),
            );
            // Every ProviderError hint points at Settings, so all of them get the button.
            let (title, detail, hint) = error.user_facing();
            settle(
                &app,
                generation,
                PopupState::error(&settings, source_kind, title, detail, hint)
                    .offering_settings(),
                &settings,
                false,
                quiet,
            );
        }
    }
}

/// Outcome of a rewrite's paste-back step.
struct Replacement {
    /// Short status for the popup's footer; empty when this was not a rewrite.
    note: String,
    /// Whether the paste failed, which forces even a quiet rewrite to show the popup.
    failed: bool,
}

/// Pastes the translation over the selection, reporting what happened.
///
/// Failure is never fatal: the popup keeps the translation and its Copy button, so a
/// read-only target or a Wayland session costs the user a keystroke, not the result.
async fn paste_over_selection(app: &AppHandle, replace: bool, translated: &str) -> Replacement {
    if !replace || translated.trim().is_empty() {
        return Replacement {
            note: String::new(),
            failed: false,
        };
    }

    let handle = app.clone();
    let text = translated.to_string();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        state.platform.replace_selection(&text)
    })
    .await;

    match outcome {
        Ok(Ok(())) => {
            log_info(app, "replaced the selection in place");
            Replacement {
                note: "REPLACED THE SELECTION".into(),
                failed: false,
            }
        }
        Ok(Err(error)) => {
            log_warn(app, &format!("could not replace the selection: {error}"));
            Replacement {
                note: "COULD NOT REPLACE — USE COPY".into(),
                failed: true,
            }
        }
        Err(join_error) => {
            log_error(app, &format!("paste task panicked: {join_error}"));
            Replacement {
                note: "COULD NOT REPLACE — USE COPY".into(),
                failed: true,
            }
        }
    }
}

/// Puts the final state on screen (unless the popup has moved on) and releases the
/// in-flight guard.
///
/// `quiet` is set for a rewrite that the user asked to happen without a popup. It is the
/// single place that decision is made, so no ending can forget it.
fn settle(
    app: &AppHandle,
    generation: u64,
    state_out: PopupState,
    settings: &Settings,
    schedule_close: bool,
    quiet: bool,
) {
    if let Some(state) = app.try_state::<AppState>() {
        if state.is_current(generation) {
            if !quiet {
                popup_window::update(app, state_out);
                if schedule_close {
                    schedule_auto_close(app, generation, settings.popup_auto_close_ms);
                }
            } else if state_out.needs_attention() {
                // Something went wrong, so the popup has to appear after all. `show`
                // rather than `update`: running quietly, no window was ever created.
                match popup_window::stored_anchor(app) {
                    Some(anchor) => {
                        popup_window::show(app, anchor, state_out);
                        popup_window::focus(app);
                    }
                    None => popup_window::update(app, state_out),
                }
            } else {
                // The rewrite already did the work; there is nothing to interrupt for.
                popup_window::close(app);
            }
        }
        state.end_flight();
    }
}

fn end_flight(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.end_flight();
    }
}

fn schedule_auto_close(app: &AppHandle, generation: u64, delay_ms: u64) {
    if delay_ms == 0 {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        // Any interaction with the popup cancels the timer.
        if state.is_current(generation) && !state.popup_was_kept() {
            popup_window::close(&app);
        }
    });
}

/// (title, detail, hint, is-fixable-in-settings)
fn describe_capture_error(
    error: &CaptureError,
    source_kind: &str,
) -> (String, String, String, bool) {
    match error {
        CaptureError::EmptySelection if source_kind == "image" => (
            "Nothing to capture".into(),
            "That region was empty or off-screen.".into(),
            "Press the hotkey again and drag across the text you want.".into(),
            false,
        ),
        CaptureError::EmptySelection => (
            "No text selected".into(),
            "Nothing was highlighted when the hotkey was pressed.".into(),
            "Select some text first, then press the hotkey again.".into(),
            false,
        ),
        CaptureError::Unsupported(detail) => (
            "Not supported on this session".into(),
            detail.clone(),
            "Settings -> Advanced lists what this session can do.".into(),
            true,
        ),
        CaptureError::Backend(detail) => (
            "Capture failed".into(),
            detail.clone(),
            "Try again; the debug log in Settings has the details.".into(),
            true,
        ),
    }
}

fn log_info(app: &AppHandle, message: &str) {
    if let Some(state) = app.try_state::<AppState>() {
        state.logger.info(message);
    }
}

fn log_warn(app: &AppHandle, message: &str) {
    if let Some(state) = app.try_state::<AppState>() {
        state.logger.warn(message);
    }
}

fn log_error(app: &AppHandle, message: &str) {
    if let Some(state) = app.try_state::<AppState>() {
        state.logger.error(message);
    }
}
