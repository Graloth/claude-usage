mod notify;
mod poller;
mod popup;
mod ring;
mod scan;
mod settings;
mod snapshot;
mod tray;

use tauri::{
    AppHandle, Emitter, Manager, RunEvent, State,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::{
    poller::Shared,
    popup::PopupState,
    scan::{ScanShared, ScanSnapshot},
    settings::{Settings, SettingsStore},
    snapshot::Snapshot,
};

#[tauri::command]
fn get_snapshot(shared: State<'_, Shared>) -> Snapshot {
    shared.snapshot()
}

#[tauri::command]
fn get_scan(scan: State<'_, ScanShared>) -> ScanSnapshot {
    scan.snapshot()
}

#[tauri::command]
fn refresh_now(shared: State<'_, Shared>, scan: State<'_, ScanShared>) {
    shared.refresh();
    scan.refresh();
}

#[tauri::command]
fn get_settings(store: State<'_, SettingsStore>) -> Settings {
    store.get()
}

/// Event carrying the new [`Settings`] after a save.
const SETTINGS_EVENT: &str = "settings-updated";

#[tauri::command]
fn save_settings(
    app: AppHandle,
    store: State<'_, SettingsStore>,
    shared: State<'_, Shared>,
    scan: State<'_, ScanShared>,
    settings: Settings,
) -> Result<Settings, String> {
    let before = store.get();
    let saved = store
        .set(settings)
        .map_err(|e| format!("could not save settings: {e}"))?;

    tray::apply(&app, &shared.snapshot(), &saved);
    // The poller re-reads settings when woken: needed for these two to apply now.
    if saved.refresh_minutes != before.refresh_minutes
        || saved.claude_config_dir != before.claude_config_dir
    {
        shared.refresh();
    }
    // Sections, config directory and interval all change what the scan does.
    scan.refresh();
    app.emit(SETTINGS_EVENT, &saved)
        .map_err(|e| e.to_string())?;
    Ok(saved)
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    // Linux (libappindicator) does not report tray clicks, so the menu is the
    // only way to open the popup there.
    let open = MenuItem::with_id(app, "open", "Show usage", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let initial = tray::view(&Snapshot::default(), &Settings::default());
    TrayIconBuilder::with_id(tray::TRAY_ID)
        .icon(tray::icon(&initial))
        // The ring is coloured, so it must not be treated as a monochrome
        // template image.
        .icon_as_template(false)
        .tooltip(&initial.tooltip)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => {
                if let Err(e) = popup::toggle(app, None) {
                    eprintln!("claude-usage: could not toggle popup: {e}");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = event
                && let Err(e) = popup::toggle(tray.app_handle(), Some(position))
            {
                eprintln!("claude-usage: could not toggle popup: {e}");
            }
        })
        .build(app)?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(PopupState::default())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            app.manage(SettingsStore::load(config_dir.join("settings.json")));

            // Tray-only app: no Dock icon or app menu on macOS.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            build_tray(app.handle())?;
            poller::start(app.handle());
            scan::start(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_scan,
            refresh_now,
            get_settings,
            save_settings,
            popup::fit_popup
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // Closing the popup removes the last window; that must not quit
            // the app. An explicit `app.exit(0)` carries a code and passes.
            if let RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                api.prevent_exit();
            }
        });
}
