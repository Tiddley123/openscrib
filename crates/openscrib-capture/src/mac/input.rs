//! The macOS keyboard and mouse hook.
//!
//! `rdev::listen` can't be used here. For every key it asks HIToolbox's text
//! input services which character was typed, and those assert that they are
//! called on the main thread, so the first keystroke of a recording traps the
//! whole app (`EXC_BREAKPOINT` in `dispatch_assert_queue`). Tauri owns the
//! main thread, so the hook has to live on a thread of its own, and the text
//! has to come from somewhere else: the window server has already worked it
//! out and attached it to the event, and reading it back is pure CoreGraphics.
//!
//! What this produces is `rdev::Event`, so the recorder turns macOS and
//! Windows input into steps through the same code.

use std::cell::Cell;
use std::time::SystemTime;

use core_foundation::base::TCFType;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField,
};
use core_graphics::event::CGKeyCode;
use core_graphics::sys::CGEventRef;
use foreign_types::ForeignType;
use rdev::{Button, Event, EventType, Key};
use tracing::warn;

unsafe extern "C" {
    /// The characters this event produced, as the window server resolved them:
    /// the layout, dead keys and modifiers are all applied already. Unlike
    /// `UCKeyTranslate` it needs no keyboard layout of ours, so there is
    /// nothing here that insists on the main thread.
    fn CGEventKeyboardGetUnicodeString(
        event: CGEventRef,
        max_length: usize,
        actual_length: *mut usize,
        unicode_string: *mut u16,
    );
    fn CGEventTapEnable(tap: *mut std::ffi::c_void, enable: bool);
}

thread_local! {
    /// The running tap's port, so the callback can switch the tap back on
    /// after the system has switched it off.
    static TAP_PORT: Cell<*mut std::ffi::c_void> = const { Cell::new(std::ptr::null_mut()) };
}

/// Install a listen-only tap on this thread and run its loop until the
/// process ends, handing every event to `on_event`. Mirrors `rdev::listen`.
///
/// Needs Accessibility: without it the tap is created but never delivers a
/// keystroke, which is why a recording checks the permission before it starts.
pub fn listen<F>(on_event: F) -> Result<(), String>
where
    F: Fn(Event) + Send + 'static,
{
    use CGEventType as T;

    let tap = CGEventTap::new(
        // The session's stream, not the HID one. Two reasons, and the first
        // is the point of this program: a remote session (TeamViewer and the
        // like) posts its operator's input as events rather than typing on
        // this Mac's hardware, and only the session stream carries those.
        // The window server has also annotated events by this point, which is
        // where the text a key produced comes from.
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        // Never alter or swallow what the user is doing.
        CGEventTapOptions::ListenOnly,
        vec![
            T::KeyDown,
            T::KeyUp,
            T::FlagsChanged,
            T::LeftMouseDown,
            T::LeftMouseUp,
            T::RightMouseDown,
            T::RightMouseUp,
            T::OtherMouseDown,
            T::OtherMouseUp,
            T::MouseMoved,
            T::LeftMouseDragged,
            T::RightMouseDragged,
            T::OtherMouseDragged,
            T::ScrollWheel,
        ],
        move |_proxy, kind, event| {
            // macOS switches a tap off if its callback is ever slow, and says
            // so through the tap itself. Switching it back on costs nothing,
            // and without it a busy moment would end the recording silently.
            if matches!(kind, T::TapDisabledByTimeout | T::TapDisabledByUserInput) {
                let port = TAP_PORT.get();
                if !port.is_null() {
                    warn!("the input tap was switched off by the system; switching it back on");
                    unsafe { CGEventTapEnable(port, true) };
                }
                return CallbackResult::Keep;
            }
            if let Some(event_type) = convert(kind, event) {
                let name = matches!(kind, T::KeyDown).then(|| typed_text(event)).flatten();
                on_event(Event { time: SystemTime::now(), name, event_type });
            }
            CallbackResult::Keep
        },
    )
    .map_err(|_| {
        "could not watch the keyboard and mouse. Grant OpenSCRIB Accessibility in \
         System Settings -> Privacy & Security."
            .to_string()
    })?;

    let source = tap
        .mach_port()
        .create_runloop_source(0)
        .map_err(|_| "could not attach the input tap to this thread".to_string())?;
    CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
    TAP_PORT.set(tap.mach_port().as_concrete_TypeRef().cast());
    tap.enable();
    CFRunLoop::run_current();
    Ok(())
}

fn keycode(event: &CGEvent) -> CGKeyCode {
    event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as CGKeyCode
}

fn convert(kind: CGEventType, event: &CGEvent) -> Option<EventType> {
    use CGEventType as T;

    Some(match kind {
        T::LeftMouseDown => EventType::ButtonPress(Button::Left),
        T::LeftMouseUp => EventType::ButtonRelease(Button::Left),
        T::RightMouseDown => EventType::ButtonPress(Button::Right),
        T::RightMouseUp => EventType::ButtonRelease(Button::Right),
        T::OtherMouseDown => EventType::ButtonPress(other_button(event)),
        T::OtherMouseUp => EventType::ButtonRelease(other_button(event)),
        // Dragging is still the pointer moving, and a drag is how plenty of
        // steps end up where they do.
        T::MouseMoved | T::LeftMouseDragged | T::RightMouseDragged | T::OtherMouseDragged => {
            let at = event.location();
            EventType::MouseMove { x: at.x, y: at.y }
        }
        T::ScrollWheel => EventType::Wheel {
            delta_x: event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2),
            delta_y: event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1),
        },
        T::KeyDown => EventType::KeyPress(key_from_code(keycode(event))),
        T::KeyUp => EventType::KeyRelease(key_from_code(keycode(event))),
        // A modifier has no press or release of its own; the event carries
        // the flags as they now stand, so its own bit says whether it went
        // down or came up.
        T::FlagsChanged => {
            let key = key_from_code(keycode(event));
            let held = flag_of(key).is_some_and(|f| event.get_flags().contains(f));
            if held { EventType::KeyPress(key) } else { EventType::KeyRelease(key) }
        }
        _ => return None,
    })
}

fn other_button(event: &CGEvent) -> Button {
    match event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER) {
        2 => Button::Middle,
        n => Button::Unknown(n.clamp(0, u8::MAX as i64) as u8),
    }
}

/// The flag a modifier key sets while it is held.
fn flag_of(key: Key) -> Option<CGEventFlags> {
    Some(match key {
        Key::ShiftLeft | Key::ShiftRight => CGEventFlags::CGEventFlagShift,
        Key::ControlLeft | Key::ControlRight => CGEventFlags::CGEventFlagControl,
        Key::Alt | Key::AltGr => CGEventFlags::CGEventFlagAlternate,
        Key::MetaLeft | Key::MetaRight => CGEventFlags::CGEventFlagCommand,
        Key::CapsLock => CGEventFlags::CGEventFlagAlphaShift,
        Key::Function => CGEventFlags::CGEventFlagSecondaryFn,
        _ => return None,
    })
}

/// What the event says was typed, or `None` for a key that types nothing.
///
/// macOS answers for every key, using its private use area for the ones that
/// produce no character: F1 comes back as `U+F704`, the arrows as `U+F700`
/// and so on. Those are not control characters, so they would otherwise end
/// up inside a typed step's text as unrenderable glyphs.
fn typed_text(event: &CGEvent) -> Option<String> {
    const PRIVATE_USE: std::ops::RangeInclusive<char> = '\u{e000}'..='\u{f8ff}';

    let mut buf = [0u16; 8];
    let mut len: usize = 0;
    unsafe {
        CGEventKeyboardGetUnicodeString(event.as_ptr(), buf.len(), &mut len, buf.as_mut_ptr());
    }
    let text: String = String::from_utf16_lossy(&buf[..len.min(buf.len())])
        .chars()
        .filter(|c| !PRIVATE_USE.contains(c))
        .collect();
    (!text.is_empty()).then_some(text)
}

/// macOS virtual key codes, which are the keyboard's own layout rather than
/// the characters on it: `KEY_A` is wherever A sits on a US keyboard, and the
/// text that key produces comes from [`typed_text`] instead.
///
/// Apple's PC-only keys (Pause, Scroll Lock, Print Screen, Num Lock) have no
/// code of their own, so they stay `Unknown` and cannot be used as hotkeys
/// here.
fn key_from_code(code: CGKeyCode) -> Key {
    match code {
        0 => Key::KeyA,
        1 => Key::KeyS,
        2 => Key::KeyD,
        3 => Key::KeyF,
        4 => Key::KeyH,
        5 => Key::KeyG,
        6 => Key::KeyZ,
        7 => Key::KeyX,
        8 => Key::KeyC,
        9 => Key::KeyV,
        10 => Key::IntlBackslash,
        11 => Key::KeyB,
        12 => Key::KeyQ,
        13 => Key::KeyW,
        14 => Key::KeyE,
        15 => Key::KeyR,
        16 => Key::KeyY,
        17 => Key::KeyT,
        18 => Key::Num1,
        19 => Key::Num2,
        20 => Key::Num3,
        21 => Key::Num4,
        22 => Key::Num6,
        23 => Key::Num5,
        24 => Key::Equal,
        25 => Key::Num9,
        26 => Key::Num7,
        27 => Key::Minus,
        28 => Key::Num8,
        29 => Key::Num0,
        30 => Key::RightBracket,
        31 => Key::KeyO,
        32 => Key::KeyU,
        33 => Key::LeftBracket,
        34 => Key::KeyI,
        35 => Key::KeyP,
        36 => Key::Return,
        37 => Key::KeyL,
        38 => Key::KeyJ,
        39 => Key::Quote,
        40 => Key::KeyK,
        41 => Key::SemiColon,
        42 => Key::BackSlash,
        43 => Key::Comma,
        44 => Key::Slash,
        45 => Key::KeyN,
        46 => Key::KeyM,
        47 => Key::Dot,
        48 => Key::Tab,
        49 => Key::Space,
        50 => Key::BackQuote,
        51 => Key::Backspace,
        53 => Key::Escape,
        54 => Key::MetaRight,
        55 => Key::MetaLeft,
        56 => Key::ShiftLeft,
        57 => Key::CapsLock,
        58 => Key::Alt,
        59 => Key::ControlLeft,
        60 => Key::ShiftRight,
        61 => Key::AltGr,
        62 => Key::ControlRight,
        63 => Key::Function,
        // The keypad, which types its own digits.
        65 => Key::KpDelete,
        67 => Key::KpMultiply,
        69 => Key::KpPlus,
        75 => Key::KpDivide,
        76 => Key::KpReturn,
        78 => Key::KpMinus,
        82 => Key::Kp0,
        83 => Key::Kp1,
        84 => Key::Kp2,
        85 => Key::Kp3,
        86 => Key::Kp4,
        87 => Key::Kp5,
        88 => Key::Kp6,
        89 => Key::Kp7,
        91 => Key::Kp8,
        92 => Key::Kp9,
        96 => Key::F5,
        97 => Key::F6,
        98 => Key::F7,
        99 => Key::F3,
        100 => Key::F8,
        101 => Key::F9,
        103 => Key::F11,
        109 => Key::F10,
        111 => Key::F12,
        114 => Key::Insert,
        115 => Key::Home,
        116 => Key::PageUp,
        117 => Key::Delete,
        118 => Key::F4,
        119 => Key::End,
        120 => Key::F2,
        121 => Key::PageDown,
        122 => Key::F1,
        123 => Key::LeftArrow,
        124 => Key::RightArrow,
        125 => Key::DownArrow,
        126 => Key::UpArrow,
        other => Key::Unknown(other.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_keys_a_hotkey_can_use() {
        // The default stop key, and the letters and digits the picker offers.
        assert_eq!(key_from_code(53), Key::Escape);
        assert_eq!(key_from_code(0), Key::KeyA);
        assert_eq!(key_from_code(29), Key::Num0);
        assert_eq!(key_from_code(122), Key::F1);
        assert_eq!(key_from_code(111), Key::F12);
        // Every modifier, so a combination can be recognised at all.
        for code in [54, 55, 56, 57, 58, 59, 60, 61, 62, 63] {
            assert!(flag_of(key_from_code(code)).is_some(), "code {code} has no modifier flag");
        }
        // Nothing unmapped in the ranges the keyboard actually uses.
        assert_eq!(key_from_code(200), Key::Unknown(200));
    }

    /// The whole point of this module: naming a keystroke must not call into
    /// HIToolbox, whose text input services trap when they are off the main
    /// thread, which is where the hook lives. `rdev` failed exactly here.
    ///
    /// A synthetic event carries no text of its own — the string is put there
    /// by the window server, which is why the tap has to sit somewhere that
    /// has already happened — so this checks the one thing it can: that
    /// asking goes through CoreGraphics and comes back.
    #[test]
    fn naming_a_keystroke_never_traps_off_the_main_thread() {
        use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

        std::thread::spawn(|| {
            let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
                .expect("event source");
            // Key code 0 is wherever A sits on the layout.
            let event = CGEvent::new_keyboard_event(source, 0, true).expect("keyboard event");
            let _ = typed_text(&event);
        })
        .join()
        .expect("naming a keystroke trapped off the main thread");
    }

    #[test]
    fn every_code_maps_to_one_key() {
        let mut seen: Vec<(Key, CGKeyCode)> = Vec::new();
        for code in 0..=126u16 {
            let key = key_from_code(code);
            if matches!(key, Key::Unknown(_)) {
                continue;
            }
            if let Some((_, first)) = seen.iter().find(|(k, _)| *k == key) {
                panic!("{key:?} is mapped from both {first} and {code}");
            }
            seen.push((key, code));
        }
    }
}
