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
    AppHandle, LogicalSize, Manager, Monitor, PhysicalPosition, PhysicalSize, State, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
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
    let size = (POPUP_WIDTH * scale, height * scale);
    place(&window, state.anchor(), size)?;
    if !window.is_visible()? {
        reveal(&window)?;
        // The OS ignores a position set on a window that has never been shown,
        // so set it again now that it is.
        place(&window, state.anchor(), size)?;
    }
    Ok(())
}

fn reveal(window: &WebviewWindow) -> tauri::Result<()> {
    window.show()?;
    window.set_focus()
}

/// A monitor's full rectangle and its usable part (without menu bar or
/// taskbar), in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MonitorArea {
    full: Rect,
    work: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Rect {
    fn new(position: PhysicalPosition<i32>, size: PhysicalSize<u32>) -> Self {
        let (left, top) = (f64::from(position.x), f64::from(position.y));
        Self {
            left,
            top,
            right: left + f64::from(size.width),
            bottom: top + f64::from(size.height),
        }
    }

    fn contains(&self, p: PhysicalPosition<f64>) -> bool {
        (self.left..self.right).contains(&p.x) && (self.top..self.bottom).contains(&p.y)
    }

    /// Squared distance from `p` to the rectangle; 0 when inside.
    fn distance_sq(&self, p: PhysicalPosition<f64>) -> f64 {
        let dx = (self.left - p.x).max(0.0).max(p.x - self.right);
        let dy = (self.top - p.y).max(0.0).max(p.y - self.bottom);
        dx * dx + dy * dy
    }
}

impl MonitorArea {
    fn of(monitor: &Monitor) -> Self {
        let work = monitor.work_area();
        Self {
            full: Rect::new(*monitor.position(), *monitor.size()),
            work: Rect::new(work.position, work.size),
        }
    }
}

/// Top-left corner for a popup of `size` (physical pixels).
///
/// The click is matched against each monitor's *full* rectangle, because a
/// tray click lands in the menu bar, which is outside the work area. A click
/// that is on no monitor (coordinate-space mismatches happen with mixed-DPI
/// setups) goes to the nearest one rather than failing. Without an anchor the
/// popup goes to the primary monitor's top-right corner.
fn popup_position(
    monitors: &[MonitorArea],
    primary: usize,
    anchor: Option<PhysicalPosition<f64>>,
    size: (f64, f64),
) -> Option<PhysicalPosition<f64>> {
    let chosen = match anchor {
        Some(p) => monitors.iter().find(|m| m.full.contains(p)).or_else(|| {
            monitors
                .iter()
                .min_by(|a, b| a.full.distance_sq(p).total_cmp(&b.full.distance_sq(p)))
        }),
        None => monitors.get(primary).or_else(|| monitors.first()),
    }?;

    let (w, h) = size;
    let Rect {
        left,
        top,
        right,
        bottom,
    } = chosen.work;
    let (x, y) = match anchor {
        // Centre on the click. Open away from the nearer screen edge: the tray
        // is at the top on macOS and most Linux desktops, at the bottom on Windows.
        Some(p) if p.y < (top + bottom) / 2.0 => (p.x - w / 2.0, p.y + MARGIN),
        Some(p) => (p.x - w / 2.0, p.y - h - MARGIN),
        None => (right - w - MARGIN, top + MARGIN),
    };

    // `max` keeps the clamp bounds ordered when the popup is larger than the area.
    Some(PhysicalPosition::new(
        x.clamp(left + MARGIN, (right - w - MARGIN).max(left + MARGIN)),
        y.clamp(top + MARGIN, (bottom - h - MARGIN).max(top + MARGIN)),
    ))
}

/// Positions a popup of `size` (physical pixels) next to `anchor`. The size is
/// passed in because the window's own reported size lags behind `set_size`.
fn place(
    window: &WebviewWindow,
    anchor: Option<PhysicalPosition<f64>>,
    size: (f64, f64),
) -> tauri::Result<()> {
    let monitors: Vec<_> = window
        .available_monitors()?
        .iter()
        .map(MonitorArea::of)
        .collect();
    let primary = window
        .primary_monitor()?
        .map(|m| MonitorArea::of(&m))
        .and_then(|p| monitors.iter().position(|m| *m == p))
        .unwrap_or(0);

    match popup_position(&monitors, primary, anchor, size) {
        Some(position) => window.set_position(position),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (f64, f64) = (760.0, 1398.0);

    fn area(left: f64, top: f64, width: f64, height: f64, menu_bar: f64) -> MonitorArea {
        let full = Rect {
            left,
            top,
            right: left + width,
            bottom: top + height,
        };
        MonitorArea {
            full,
            work: Rect {
                top: top + menu_bar,
                ..full
            },
        }
    }

    /// The two displays of the machine this was debugged on: a 4112x2658
    /// primary and a 3840x2160 display to its left.
    fn desk() -> [MonitorArea; 2] {
        [
            area(0.0, 0.0, 4112.0, 2658.0, 78.0),
            area(-3840.0, 0.0, 3840.0, 2160.0, 60.0),
        ]
    }

    fn at(x: f64, y: f64) -> Option<PhysicalPosition<f64>> {
        Some(PhysicalPosition::new(x, y))
    }

    fn pos(p: Option<PhysicalPosition<f64>>) -> (f64, f64) {
        let p = p.expect("a position");
        (p.x, p.y)
    }

    #[test]
    fn tray_click_in_the_menu_bar_opens_below_it_centred() {
        // The click that failed in practice: inside the monitor, above the work area.
        let p = popup_position(&desk(), 0, at(2544.0, 27.0), SIZE);
        assert_eq!(pos(p), (2544.0 - 380.0, 86.0));
    }

    #[test]
    fn clicks_near_the_right_edge_are_pulled_inside() {
        let p = popup_position(&desk(), 0, at(4100.0, 27.0), SIZE);
        assert_eq!(pos(p).0, 4112.0 - 760.0 - MARGIN);
    }

    #[test]
    fn a_click_on_the_second_monitor_stays_there() {
        let p = pos(popup_position(&desk(), 0, at(-1000.0, 20.0), SIZE));
        assert!((-3840.0..0.0).contains(&p.0), "x = {}", p.0);
        assert_eq!(p.1, 60.0 + MARGIN);
    }

    #[test]
    fn a_click_on_no_monitor_goes_to_the_nearest_one() {
        // Beyond the right edge of the primary monitor.
        let p = pos(popup_position(&desk(), 0, at(9000.0, 27.0), SIZE));
        assert_eq!(p.0, 4112.0 - 760.0 - MARGIN);
        // Far to the left of the second monitor.
        let p = pos(popup_position(&desk(), 0, at(-9000.0, 27.0), SIZE));
        assert_eq!(p.0, -3840.0 + MARGIN);
    }

    #[test]
    fn a_taskbar_at_the_bottom_opens_upwards() {
        let monitors = [area(0.0, 0.0, 1920.0, 1080.0, 0.0)];
        let p = pos(popup_position(
            &monitors,
            0,
            at(1800.0, 1070.0),
            (760.0, 600.0),
        ));
        assert_eq!(p.1, 1070.0 - 600.0 - MARGIN);
    }

    #[test]
    fn without_an_anchor_it_goes_top_right_of_the_primary() {
        let p = pos(popup_position(&desk(), 0, None, SIZE));
        assert_eq!(p, (4112.0 - 760.0 - MARGIN, 78.0 + MARGIN));
        // Another monitor can be primary.
        let p = pos(popup_position(&desk(), 1, None, SIZE));
        assert_eq!(p.0, -SIZE.0 - MARGIN);
    }

    #[test]
    fn a_popup_taller_than_the_screen_does_not_panic() {
        let monitors = [area(0.0, 0.0, 800.0, 500.0, 0.0)];
        let p = pos(popup_position(
            &monitors,
            0,
            at(400.0, 10.0),
            (900.0, 900.0),
        ));
        assert_eq!(p, (MARGIN, MARGIN));
    }

    #[test]
    fn no_monitors_means_no_position() {
        assert!(popup_position(&[], 0, at(1.0, 1.0), SIZE).is_none());
    }
}
