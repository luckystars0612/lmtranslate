// No console window on Windows release builds — this app lives in the tray.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod commands;
mod config;
mod demo;
mod flows;
mod hotkeys;
mod logger;
mod platform;
mod state;
mod translate;
mod windows;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_autostart::MacosLauncher;

use crate::config::Settings;
use crate::logger::Logger;
use crate::state::AppState;
use crate::windows::settings_window;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    hotkeys::on_shortcut(app, shortcut, event.state());
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::list_providers,
            commands::list_languages,
            commands::check_hotkey,
            commands::begin_hotkey_capture,
            commands::end_hotkey_capture,
            commands::test_connection,
            commands::platform_report,
            commands::app_theme,
            commands::open_settings,
            commands::get_popup_state,
            commands::close_popup,
            commands::keep_popup_open,
            commands::resize_popup,
            commands::retranslate,
            commands::copy_text,
            commands::crop_commit,
            commands::crop_cancel,
            commands::get_log,
            commands::clear_log,
            commands::reveal_log,
            commands::open_accessibility_settings,
        ])
        .setup(setup)
        .build(tauri::generate_context!())
        .expect("LM Translate failed to start")
        .run(|_app, event| {
            // There is no main window: dismissing the popup or closing Settings must
            // not take the app down with it. A code means someone asked to quit.
            if let RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();

    let config_path = config::config_path(&handle)?;
    let logger = Logger::new(config::log_path(&handle)?);

    let settings = match config::load(&config_path) {
        Ok(settings) => settings,
        Err(e) => {
            logger.warn(format!("using default settings: {e}"));
            Settings::default()
        }
    };

    // One platform backend, chosen once, from here on hidden behind the trait.
    let platform = platform::detect();
    let capabilities = platform.capabilities();
    logger.info(format!(
        "LM Translate {} starting — {}",
        env!("CARGO_PKG_VERSION"),
        capabilities.backend
    ));
    if !capabilities.supports_text_selection {
        logger.warn("select-and-translate is not available on this session");
    }

    let provider = translate::build_provider(&settings)
        .map_err(|e| format!("could not build the translation provider: {e}"))?;

    app.manage(AppState::new(
        config_path,
        settings.clone(),
        platform,
        provider,
        logger,
    ));

    build_tray(&handle)?;

    for problem in hotkeys::apply(&handle, &settings) {
        if let Some(state) = handle.try_state::<AppState>() {
            state.logger.warn(&problem);
        }
    }

    commands::apply_autostart(&handle, settings.launch_on_login);

    // `lmtranslate --settings` opens Settings straight away, so the app can be given a
    // desktop shortcut that goes somewhere useful instead of only living in the tray.
    if std::env::args().any(|arg| arg == "--settings") {
        settings_window::open(&handle)?;
    }

    // `lmtranslate --self-test` cycles the popup through all three states at every
    // screen edge and probes both capture backends — the parts of the milestone
    // acceptance tests that need no API key and no mouse.
    if std::env::args().any(|arg| arg == "--self-test") {
        demo::run(handle.clone());
    }

    // No dock icon on macOS: this is a background utility.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let translate_selection = MenuItem::with_id(
        app,
        "translate-selection",
        "Translate selection",
        true,
        None::<&str>,
    )?;
    let translate_region = MenuItem::with_id(
        app,
        "translate-region",
        "Translate screen region",
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let settings_item = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit LM Translate", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &translate_selection,
            &translate_region,
            &separator,
            &settings_item,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("LM Translate")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "translate-selection" => flows::start_text_flow(app.clone()),
            "translate-region" => flows::start_screenshot_flow(app.clone()),
            "settings" => {
                if let Err(e) = settings_window::open(app) {
                    eprintln!("could not open Settings: {e}");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}
