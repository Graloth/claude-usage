//! The usage popup: a borderless, transparent webview window that exists only
//! while it is open.
//!
//! It is created hidden, sized by the page once the first content has been
//! laid out ([`fit_popup`]), then shown next to the tray icon. Showing it only
//! after the first fit avoids a flash of an empty, wrongly sized window.

use std::{
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use crate::{poller::Shared, scan::ScanShared};

const POPUP_LABEL: &str = "popup";
const POPUP_WIDTH: f64 = 380.0;
/// Height bounds in logical pixels. Content taller than the maximum scrolls.
const POPUP_MIN_HEIGHT: f64 = 120.0;
const POPUP_MAX_HEIGHT: f64 = 720.0;
/// Gap between the popup and the screen edge / tray icon, in physical pixels.
const MARGIN: f64 = 8.0;
/// A tray click on an open popup blurs (and so closes) it first. Clicks
/// arriving this soon after a blur-close are the same gesture and are ignored,
/// so clicking the icon toggles instead of closing and instantly reopening.
const REOPEN_GUARD: Duration = Duration::from_millis(300);
/// If the page never reports its size (script error), show the popup anyway.
const SHOW_FALLBACK: Duration = Duration::from_millis(1500);

#[derive(Default)]
pub struct PopupState {
    closed_at: Mutex<Option<Instant>>,
    /// Where the user clicked; the popup is placed relative to it.
    anchor: Mutex<Option<PhysicalPosition<f64>>>,
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

    fn set_anchor(&self, anchor: Option<PhysicalPosition<f64>>) {
        if let Ok(mut slot) = self.anchor.lock() {
            *slot = anchor;
        }
    }

    fn anchor(&self) -> Option<PhysicalPosition<f64>> {
        self.anchor.lock().ok().and_then(|slot| *slot)
    }
}

/// Opens the popup, or closes it if it is already open.
///
/// The webview is created on demand and destroyed on blur rather than hidden,
/// so no webview process exists while the app idles in the tray. `anchor` is
/// where the user clicked; platforms that do not report tray clicks pass
/// `None` and get the top-right corner of the primary monitor.
pub fn toggle(app: &AppHandle, anchor: Option<PhysicalPosition<f64>>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(POPUP_LABEL) {
        return window.close();
    }
    let state = app.state::<PopupState>();
    if state.just_closed() {
        return Ok(());
    }
    state.set_anchor(anchor);

    let window = WebviewWindowBuilder::new(app, POPUP_LABEL, WebviewUrl::App("index.html".into()))
        .title("Claude usage")
        .inner_size(POPUP_WIDTH, POPUP_MIN_HEIGHT)
        .resizable(false)
        .decorations(false)
        // The page draws its own rounded panel on a transparent window.
        .transparent(true)
        .shadow(false)
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

    let handle = app.clone();
    thread::spawn(move || {
        thread::sleep(SHOW_FALLBACK);
        if let Some(window) = handle.get_webview_window(POPUP_LABEL)
            && matches!(window.is_visible(), Ok(false))
        {
            let _ = reveal(&window);
        }
    });

    // Opening the popup is a good moment for fresher numbers; the poller still
    // enforces its minimum gap between requests.
    app.state::<Shared>().refresh();
    app.state::<ScanShared>().refresh();
    Ok(())
}

/// Called by the page whenever its content height changes: resizes the popup
/// to fit, keeps it anchored to the tray, and shows it on the first call.
#[tauri::command]
pub fn fit_popup(
    window: WebviewWindow,
    state: State<'_, PopupState>,
    height: f64,
) -> tauri::Result<()> {
    let height = height.clamp(POPUP_MIN_HEIGHT, POPUP_MAX_HEIGHT);
    window.set_size(LogicalSize::new(POPUP_WIDTH, height))?;

    let scale = window.scale_factor()?;
    place(
        &window,
        state.anchor(),
        (POPUP_WIDTH * scale, height * scale),
    )?;
    if window.is_visible()? {
        Ok(())
    } else {
        reveal(&window)
    }
}

fn reveal(window: &WebviewWindow) -> tauri::Result<()> {
    window.show()?;
    window.set_focus()
}

/// Positions a popup of `size` (physical pixels) next to `anchor`, kept inside
/// the monitor's work area. The size is passed in because the window's own
/// reported size lags behind a `set_size` call.
fn place(
    window: &WebviewWindow,
    anchor: Option<PhysicalPosition<f64>>,
    size: (f64, f64),
) -> tauri::Result<()> {
    let monitor = match anchor {
        Some(p) => window.monitor_from_point(p.x, p.y)?,
        None => window.primary_monitor()?,
    };
    let Some(monitor) = monitor else {
        return Ok(());
    };

    let (w, h) = size;
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
