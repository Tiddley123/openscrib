//! macOS window lookups, cursor position and the privacy permissions a
//! recording needs.
//!
//! macOS reports windows, clicks and the cursor in points, while a screen
//! capture comes back in physical pixels. Everything above this module works
//! in pixels, as it does on Windows, so coordinates are converted here: a
//! point on a display with scale `s` becomes `point * s`. Each monitor's
//! origin and size scale the same way, so a click's offset into the frame
//! captured from that monitor stays exact.

use std::ffi::c_void;

use core_foundation::base::TCFType;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_graphics::access::ScreenCaptureAccess;
use core_graphics::display::{CGDisplay, CGRect};
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
    kCGWindowOwnerName,
};
use openscrib_core::{Point, Rect, WindowInfo};

unsafe extern "C" {
    fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect) -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

/// One display in both coordinate spaces.
struct Display {
    /// Point coordinates, as macOS reports them.
    bounds: CGRect,
    scale: f64,
}

impl Display {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.bounds.origin.x
            && y >= self.bounds.origin.y
            && x < self.bounds.origin.x + self.bounds.size.width
            && y < self.bounds.origin.y + self.bounds.size.height
    }
}

fn displays() -> Vec<Display> {
    let ids = CGDisplay::active_displays().unwrap_or_default();
    ids.into_iter()
        .map(|id| {
            let d = CGDisplay::new(id);
            let bounds = d.bounds();
            let scale = d
                .display_mode()
                .filter(|m| m.width() > 0)
                .map(|m| m.pixel_width() as f64 / m.width() as f64)
                .filter(|s| *s > 0.0)
                .unwrap_or(1.0);
            Display { bounds, scale }
        })
        .collect()
}

/// The scale of the display holding a point, or the main display's scale for
/// a point in the gap between displays.
fn scale_at(x: f64, y: f64) -> f64 {
    let all = displays();
    all.iter()
        .find(|d| d.contains(x, y))
        .or_else(|| all.first())
        .map_or(1.0, |d| d.scale)
}

/// Points (what macOS reports) to pixels (what the rest of the code uses).
pub fn to_pixels(x: f64, y: f64) -> Point {
    let s = scale_at(x, y);
    Point::new((x * s).round() as i32, (y * s).round() as i32)
}

/// Pixels back to points, for the macOS calls that take them. Mixed-scale
/// setups can leave a pixel coordinate inside two displays' ranges; the
/// first active display wins, which is the main one in practice.
pub fn to_points(p: Point) -> (f64, f64) {
    let (x, y) = (f64::from(p.x), f64::from(p.y));
    for d in displays() {
        let s = d.scale;
        if d.contains(x / s, y / s) {
            return (x / s, y / s);
        }
    }
    (x, y)
}

/// Scale a monitor or window rectangle from points to pixels.
pub fn rect_to_pixels(r: CGRect) -> Rect {
    let s = scale_at(
        r.origin.x + r.size.width / 2.0,
        r.origin.y + r.size.height / 2.0,
    );
    Rect {
        x: (r.origin.x * s).round() as i32,
        y: (r.origin.y * s).round() as i32,
        w: (r.size.width * s).round().max(0.0) as u32,
        h: (r.size.height * s).round().max(0.0) as u32,
    }
}

unsafe fn value(dict: CFDictionaryRef, key: CFStringRef) -> Option<*const c_void> {
    let v = unsafe { CFDictionaryGetValue(dict, key.cast()) };
    (!v.is_null()).then_some(v)
}

unsafe fn number(dict: CFDictionaryRef, key: CFStringRef) -> Option<f64> {
    let v = unsafe { value(dict, key)? };
    unsafe { CFNumber::wrap_under_get_rule(v.cast()) }.to_f64()
}

unsafe fn text(dict: CFDictionaryRef, key: CFStringRef) -> Option<String> {
    let v = unsafe { value(dict, key)? };
    Some(unsafe { CFString::wrap_under_get_rule(v.cast()) }.to_string())
}

unsafe fn bounds(dict: CFDictionaryRef) -> Option<CGRect> {
    let v = unsafe { value(dict, kCGWindowBounds)? };
    let mut r = CGRect::default();
    unsafe { CGRectMakeWithDictionaryRepresentation(v.cast(), &mut r) }.then_some(r)
}

fn describe(dict: CFDictionaryRef, rect: CGRect) -> WindowInfo {
    WindowInfo {
        rect: rect_to_pixels(rect),
        // macOS has no client-area call; the whole window is the content.
        client: None,
        // The title needs Screen Recording permission, so it can be empty
        // even when the window is there.
        title: unsafe { text(dict, kCGWindowName) }.unwrap_or_default(),
        // The app's name ("Safari", "TeamViewer"), macOS's closest match to
        // an executable name.
        process: unsafe { text(dict, kCGWindowOwnerName) }.unwrap_or_default(),
    }
}

/// Visit on-screen windows front to back, skipping the desktop picture,
/// invisible windows and anything with no area.
fn each_window(mut f: impl FnMut(CFDictionaryRef, CGRect) -> bool) {
    let Some(list) = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        return;
    };
    for item in list.iter() {
        let dict = (*item).cast::<core_foundation_sys::dictionary::__CFDictionary>();
        if dict.is_null() {
            continue;
        }
        if unsafe { number(dict, kCGWindowLayer) }.unwrap_or(0.0) < 0.0 {
            continue;
        }
        if unsafe { number(dict, kCGWindowAlpha) }.unwrap_or(1.0) <= 0.0 {
            continue;
        }
        let Some(rect) = (unsafe { bounds(dict) }) else { continue };
        if rect.size.width < 1.0 || rect.size.height < 1.0 {
            continue;
        }
        if f(dict, rect) {
            return;
        }
    }
}

/// The frontmost window under a screen point (the click target). The Dock and
/// menu bar count: a click on them is a real step.
pub fn window_at(p: Point) -> Option<WindowInfo> {
    let (x, y) = to_points(p);
    let mut found = None;
    each_window(|dict, rect| {
        let inside = x >= rect.origin.x
            && y >= rect.origin.y
            && x < rect.origin.x + rect.size.width
            && y < rect.origin.y + rect.size.height;
        if inside {
            found = Some(describe(dict, rect));
        }
        inside
    });
    found
}

/// The frontmost ordinary window, ignoring the Dock, menu bar and overlays.
pub fn foreground_window() -> Option<WindowInfo> {
    let mut found = None;
    each_window(|dict, rect| {
        if unsafe { number(dict, kCGWindowLayer) }.unwrap_or(0.0) != 0.0 {
            return false;
        }
        found = Some(describe(dict, rect));
        true
    });
    found
}

pub fn cursor_pos() -> Option<Point> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
    let at = CGEvent::new(source).ok()?.location();
    Some(to_pixels(at.x, at.y))
}

/// Screen Recording, in System Settings -> Privacy & Security. Without it
/// captures come back blank and window titles are empty.
pub fn screen_recording_granted() -> bool {
    ScreenCaptureAccess.preflight()
}

/// Ask for Screen Recording. macOS shows its prompt only the first time a
/// build asks; after that this just reports the current answer, so offer
/// `open_privacy_pane` as well.
pub fn request_screen_recording() -> bool {
    ScreenCaptureAccess.request()
}

/// Accessibility, in System Settings -> Privacy & Security. Without it the
/// input hook sees no clicks or keystrokes at all.
pub fn accessibility_granted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Ask for Accessibility, which opens macOS's own prompt with a button to
/// System Settings.
pub fn request_accessibility() -> bool {
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;

    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options = CFDictionary::from_CFType_pairs(&[(
        key.as_CFType(),
        CFBoolean::true_value().as_CFType(),
    )]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}

/// Open the Privacy & Security pane for `anchor`, e.g. `Privacy_Accessibility`
/// or `Privacy_ScreenCapture`.
pub fn open_privacy_pane(anchor: &str) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(format!("x-apple.systempreferences:com.apple.preference.security?{anchor}"))
        .status()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_and_point_spaces_round_trip() {
        // Whatever this machine's displays are, a point mapped to pixels and
        // back must land where it started.
        for (x, y) in [(0.0, 0.0), (100.0, 80.0), (640.0, 400.0)] {
            let px = to_pixels(x, y);
            let (bx, by) = to_points(px);
            assert!((bx - x).abs() < 1.0 && (by - y).abs() < 1.0, "{x},{y} -> {px:?} -> {bx},{by}");
        }
    }

    #[test]
    fn monitor_rects_scale_with_their_origin() {
        for d in displays() {
            let r = rect_to_pixels(d.bounds);
            assert_eq!(r.x, (d.bounds.origin.x * d.scale).round() as i32);
            assert_eq!(r.w, (d.bounds.size.width * d.scale).round() as u32);
        }
    }
}
