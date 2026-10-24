//! The notch window: a transparent surface covering the whole screen, above everything, that
//! takes clicks only inside the pill (its input region); everywhere else they fall through. The
//! page places the pill anywhere on it, so dragging the notch is plain CSS with no compositor
//! round trip.
//!
//! On Wayland compositors with wlr-layer-shell (KDE Plasma, Hyprland, Sway, COSMIC) it is an
//! overlay layer surface. GNOME has none, so there the app runs through `XWayland` as an
//! always-on-top dock window. Layer-shell handling adapted from Coucou (MIT, © 2026 Louis Raillé),
//! `windows/src-tauri/src/platform/linux.rs`.

use std::sync::Mutex;

use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use tauri::{Emitter, WebviewWindow};

/// No screen is larger; anything beyond is a bogus value from the page.
const MAX_SIDE: f64 = 16_384.0;

/// The few gtk-layer-shell calls needed, straight from the C library.
mod layer {
    use gtk::ffi::GtkWindow;
    use std::os::raw::{c_char, c_int};

    pub const LAYER_OVERLAY: c_int = 3;
    pub const EDGE_LEFT: c_int = 0;
    pub const EDGE_RIGHT: c_int = 1;
    pub const EDGE_TOP: c_int = 2;
    pub const EDGE_BOTTOM: c_int = 3;
    pub const KEYBOARD_NONE: c_int = 0;

    #[link(name = "gtk-layer-shell")]
    unsafe extern "C" {
        pub fn gtk_layer_is_supported() -> c_int;
        pub fn gtk_layer_init_for_window(window: *mut GtkWindow);
        pub fn gtk_layer_set_namespace(window: *mut GtkWindow, name_space: *const c_char);
        pub fn gtk_layer_set_layer(window: *mut GtkWindow, layer: c_int);
        pub fn gtk_layer_set_anchor(window: *mut GtkWindow, edge: c_int, anchor: c_int);
        pub fn gtk_layer_set_exclusive_zone(window: *mut GtkWindow, zone: c_int);
        pub fn gtk_layer_set_keyboard_mode(window: *mut GtkWindow, mode: c_int);
    }
}

type Rect = (i32, i32, i32, i32);

/// The pill's rectangle, re-applied on every map: GTK resets the input region then.
/// Empty until the page reports its shape, so nothing takes the mouse before that.
static REGION: Mutex<Rect> = Mutex::new((0, 0, 0, 0));

/// GNOME on Wayland ignores where a window asks to go; through `XWayland` it can be placed.
/// Only when `XWayland` is there (`DISPLAY`), or GTK would be left with no display at all.
fn should_prefer_x11(session: &str, desktop: &str, display: &str, backend: &str) -> bool {
    session.eq_ignore_ascii_case("wayland")
        && desktop.split(':').any(|d| d.eq_ignore_ascii_case("gnome"))
        && !display.trim().is_empty()
        && backend.is_empty()
}

/// Must run before GTK starts.
pub fn prefer_x11_on_gnome() {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    if should_prefer_x11(&env("XDG_SESSION_TYPE"), &env("XDG_CURRENT_DESKTOP"), &env("DISPLAY"), &env("GDK_BACKEND")) {
        // SAFETY: called first thing in main, before any other thread exists.
        unsafe { std::env::set_var("GDK_BACKEND", "x11") };
    }
}

/// Validates a rectangle from the page (logical pixels).
pub fn clamp_region(x: f64, y: f64, w: f64, h: f64) -> Option<Rect> {
    if ![x, y, w, h].iter().all(|v| v.is_finite()) || w < 0.0 || h < 0.0 {
        return None;
    }
    let x0 = x.floor().clamp(0.0, MAX_SIDE);
    let y0 = y.floor().clamp(0.0, MAX_SIDE);
    let x1 = (x + w).ceil().clamp(x0, MAX_SIDE);
    let y1 = (y + h).ceil().clamp(y0, MAX_SIDE);
    Some((x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
}

fn apply(gw: &impl IsA<gtk::Widget>, (x, y, w, h): Rect) {
    if let Some(window) = gw.window() {
        let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(x, y, w, h));
        window.input_shape_combine_region(&region, 0, 0);
    }
}

fn region() -> Rect {
    *REGION.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn set_region(win: &WebviewWindow, rect: Rect) -> Result<(), String> {
    *REGION.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = rect;
    let w = win.clone();
    win.run_on_main_thread(move || {
        if let Ok(gw) = w.gtk_window() {
            apply(&gw, rect);
        }
    })
    .map_err(|e| e.to_string())
}

/// Turns the hidden window into the notch, then shows it. Main thread, before the first show:
/// a layer surface cannot be made from a window the compositor already knows.
pub fn setup(win: &WebviewWindow) -> Result<(), String> {
    let gw = win.gtk_window().map_err(|e| e.to_string())?;
    gw.set_accept_focus(false);
    // The input region is only the pill, so the pointer entering or leaving the surface is the
    // pointer entering or leaving the pill. WebKit reports neither reliably here (it keeps the pill
    // hovered after the pointer left, and sends no event when it comes straight back), so the
    // page hears it from GTK: `pointer` with true or false.
    gw.add_events(gtk::gdk::EventMask::ENTER_NOTIFY_MASK | gtk::gdk::EventMask::LEAVE_NOTIFY_MASK);
    for inside in [true, false] {
        let page = win.clone();
        let crossed = move |_: &gtk::ApplicationWindow, event: &gtk::gdk::EventCrossing| {
            // Inferior: between the window and a child of it, still inside the surface.
            if event.detail() != gtk::gdk::NotifyType::Inferior {
                let _ = page.emit("pointer", inside);
            }
            gtk::glib::Propagation::Proceed
        };
        if inside {
            gw.connect_enter_notify_event(crossed);
        } else {
            gw.connect_leave_notify_event(crossed);
        }
    }
    let wanted = std::env::var("CLAUDE_NOTCH_LAYER_SHELL").map_or(true, |v| v != "0");
    // SAFETY: a query with no arguments.
    let supported = unsafe { layer::gtk_layer_is_supported() } != 0;
    if wanted && supported && !gw.is_realized() {
        // tao gives undecorated Wayland windows an empty titlebar; a layer surface has none,
        // and a client-decorated window would recompute its own input region on every map.
        gw.set_titlebar(None::<&gtk::Widget>);
        let ptr: *mut gtk::ffi::GtkWindow = gw.upcast_ref::<gtk::Window>().to_glib_none().0;
        // SAFETY: `ptr` is the live, not yet realized GtkWindow owned by `gw` for this whole block.
        unsafe {
            layer::gtk_layer_init_for_window(ptr);
            layer::gtk_layer_set_namespace(ptr, c"claude-notch".as_ptr());
            layer::gtk_layer_set_layer(ptr, layer::LAYER_OVERLAY);
            // All four edges: the whole output, so the pill can go anywhere on it.
            for edge in [layer::EDGE_LEFT, layer::EDGE_RIGHT, layer::EDGE_TOP, layer::EDGE_BOTTOM] {
                layer::gtk_layer_set_anchor(ptr, edge, 1);
            }
            // -1: over any panel, so at the top it sits flush against the screen edge like a notch.
            layer::gtk_layer_set_exclusive_zone(ptr, -1);
            layer::gtk_layer_set_keyboard_mode(ptr, layer::KEYBOARD_NONE);
        }
        // WebKitGTK in a freshly mapped layer surface may never paint its first frame;
        // unmapping and mapping it once right after the first map gets it drawing.
        let remapped = std::cell::Cell::new(false);
        gw.connect_map_event(move |w, _| {
            apply(w, region());
            if !remapped.replace(true) {
                let w = w.clone();
                gtk::glib::idle_add_local_once(move || {
                    w.hide();
                    w.show_all();
                    apply(&w, region());
                });
            }
            gtk::glib::Propagation::Proceed
        });
        crate::log("notch is a layer-shell overlay");
    } else {
        gw.set_type_hint(gtk::gdk::WindowTypeHint::Dock);
        gw.set_keep_above(true);
        gw.connect_map_event(|w, _| {
            apply(w, region());
            gtk::glib::Propagation::Proceed
        });
        if let Ok(Some(monitor)) = win.primary_monitor() {
            let _ = win.set_resizable(true);
            let _ = win.set_size(*monitor.size());
            let _ = win.set_position(*monitor.position());
        }
        crate::log("notch is an always-on-top window (no layer-shell here)");
    }
    win.show().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_only_on_gnome_wayland_with_xwayland_and_no_override() {
        assert!(should_prefer_x11("wayland", "ubuntu:GNOME", ":0", ""));
        assert!(!should_prefer_x11("wayland", "KDE", ":0", ""));
        assert!(!should_prefer_x11("x11", "GNOME", ":0", ""));
        assert!(!should_prefer_x11("wayland", "GNOME", "", ""));
        assert!(!should_prefer_x11("wayland", "GNOME", ":0", "wayland"));
    }

    #[test]
    fn regions_are_validated_and_clamped() {
        assert_eq!(clamp_region(100.4, 0.0, 200.2, 36.0), Some((100, 0, 201, 36)));
        assert_eq!(clamp_region(-10.0, -5.0, 2000.0, 900.0), Some((0, 0, 1990, 895)));
        assert_eq!(clamp_region(20_000.0, 0.0, 10.0, 10.0), Some((16_384, 0, 0, 10)));
        assert_eq!(clamp_region(f64::NAN, 0.0, 1.0, 1.0), None);
        assert_eq!(clamp_region(0.0, 0.0, f64::INFINITY, 1.0), None);
        assert_eq!(clamp_region(0.0, 0.0, -1.0, 1.0), None);
    }
}
