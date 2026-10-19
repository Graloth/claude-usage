mod poller;
mod ring;
mod snapshot;
mod tray;

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use tauri::{
    AppHandle, Manager, PhysicalPosition, RunEvent, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::{poller::Shared, snapshot::Snapshot};

const POPUP_LABEL: &str = "popup";
const POPUP_SIZE: (f64, f64) = (380.0, 560.0);
/// Gap between the popup and the screen edge / tray icon, in physical pixels.
const MARGIN: f64 = 8.0;
/// A tray click on an open popup blurs (and so closes) it first. Clicks
/// arriving this soon after a blur-close are the same gesture and are ignored,
/// so clicking the icon toggles instead of closing and instantly reopening.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

#[derive(Default)]
struct PopupState {
    closed_at: Mutex<Option<Instant>>,
}

impl PopupState {
    fn mark_closed(&self) {
        if let Ok(mut closed_at) = self.closed_at.lock() {
            *closed_at = Some(Instant::now());
        }
    }

    fn just_closed(&self) -> bool {
        self.closed_at
            .lock()
            .ok()
            .and_then(|closed_at| *closed_at)
            .is_some_and(|at| at.elapsed() < REOPEN_GUARD)
    }
}

/// Opens the popup, or closes it if it is already open.
///
/// The webview is created on demand and destroyed on blur rather than hidden,
/// so no webview process exists while the app idles in the tray. `anchor` is
/// where the user clicked; platforms that do not report tray clicks pass
/// `None` and get the top-right corner of the primary monitor.
fn toggle_popup(app: &AppHandle, anchor: Option<PhysicalPosition<f64>>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(POPUP_LABEL) {
        return window.close();
    }
    if app.state::<PopupState>().just_closed() {
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(app, POPUP_LABEL, WebviewUrl::App("index.html".into()))
        .title("Claude usage")
        .inner_size(POPUP_SIZE.0, POPUP_SIZE.1)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()?;

    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            handle.state::<PopupState>().mark_closed();
            if let Some(window) = handle.get_webview_window(POPUP_LABEL) {
                let _ = window.close();
            }
        }
    });

    place_popup(&window, anchor)?;
    window.show()?;
    // Opening the popup is a good moment for fresher numbers; the poller still
    // enforces its minimum gap between requests.
    app.state::<Shared>().refresh();
    window.set_focus()
}

#[tauri::command]
fn get_snapshot(shared: State<'_, Shared>) -> Snapshot {
    shared.snapshot()
}

#[tauri::command]
fn refresh_now(shared: State<'_, Shared>) {
    shared.refresh();
}

/// Positions the popup next to `anchor`, kept inside the monitor's work area.
fn place_popup(window: &WebviewWindow, anchor: Option<PhysicalPosition<f64>>) -> tauri::Result<()> {
    let monitor = match anchor {
        Some(p) => window.monitor_from_point(p.x, p.y)?,
        None => window.primary_monitor()?,
    };
    let Some(monitor) = monitor else {
        return Ok(());
    };

    let size = window.outer_size()?;
    let (w, h) = (f64::from(size.width), f64::from(size.height));
    let area = monitor.work_area();
    let (left, top) = (f64::from(area.position.x), f64::from(area.position.y));
    let right = left + f64::from(area.size.width);
    let bottom = top + f64::from(area.size.height);

    let (x, y) = match anchor {
        // Centre on the click. Open away from the nearer screen edge: the tray
        // is at the top on macOS and most Linux desktops, at the bottom on Windows.
        Some(p) if p.y < (top + bottom) / 2.0 => (p.x - w / 2.0, p.y + MARGIN),
        Some(p) => (p.x - w / 2.0, p.y - h - MARGIN),
        None => (right - w - MARGIN, top + MARGIN),
    };

    // `max` keeps the clamp bounds ordered when the popup is larger than the area.
    let x = x.clamp(left + MARGIN, (right - w - MARGIN).max(left + MARGIN));
    let y = y.clamp(top + MARGIN, (bottom - h - MARGIN).max(top + MARGIN));
    window.set_position(PhysicalPosition::new(x, y))
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    // Linux (libappindicator) does not report tray clicks, so the menu is the
    // only way to open the popup there.
    let open = MenuItem::with_id(app, "open", "Show usage", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let initial = tray::view(&Snapshot::default());
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
                if let Err(e) = toggle_popup(app, None) {
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
                && let Err(e) = toggle_popup(tray.app_handle(), Some(position))
            {
                eprintln!("claude-usage: could not toggle popup: {e}");
            }
        })
        .build(app)?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .manage(PopupState::default())
        .setup(|app| {
            // Tray-only app: no Dock icon or app menu on macOS.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            build_tray(app.handle())?;
            poller::start(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_snapshot, refresh_now])
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
