//! The key combination that stops a recording, written like `Ctrl+Shift+F12`.

use std::fmt;

use rdev::Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: Key,
}

impl Default for Hotkey {
    fn default() -> Self {
        Self { ctrl: true, alt: false, shift: true, win: false, key: Key::F12 }
    }
}

const NAMED: &[(&str, Key)] = &[
    ("F1", Key::F1),
    ("F2", Key::F2),
    ("F3", Key::F3),
    ("F4", Key::F4),
    ("F5", Key::F5),
    ("F6", Key::F6),
    ("F7", Key::F7),
    ("F8", Key::F8),
    ("F9", Key::F9),
    ("F10", Key::F10),
    ("F11", Key::F11),
    ("F12", Key::F12),
    ("Pause", Key::Pause),
    ("ScrollLock", Key::ScrollLock),
    ("PrintScreen", Key::PrintScreen),
    ("Insert", Key::Insert),
    ("Delete", Key::Delete),
    ("Home", Key::Home),
    ("End", Key::End),
    ("PageUp", Key::PageUp),
    ("PageDown", Key::PageDown),
    ("Esc", Key::Escape),
    ("Space", Key::Space),
    ("A", Key::KeyA),
    ("B", Key::KeyB),
    ("C", Key::KeyC),
    ("D", Key::KeyD),
    ("E", Key::KeyE),
    ("F", Key::KeyF),
    ("G", Key::KeyG),
    ("H", Key::KeyH),
    ("I", Key::KeyI),
    ("J", Key::KeyJ),
    ("K", Key::KeyK),
    ("L", Key::KeyL),
    ("M", Key::KeyM),
    ("N", Key::KeyN),
    ("O", Key::KeyO),
    ("P", Key::KeyP),
    ("Q", Key::KeyQ),
    ("R", Key::KeyR),
    ("S", Key::KeyS),
    ("T", Key::KeyT),
    ("U", Key::KeyU),
    ("V", Key::KeyV),
    ("W", Key::KeyW),
    ("X", Key::KeyX),
    ("Y", Key::KeyY),
    ("Z", Key::KeyZ),
    ("0", Key::Num0),
    ("1", Key::Num1),
    ("2", Key::Num2),
    ("3", Key::Num3),
    ("4", Key::Num4),
    ("5", Key::Num5),
    ("6", Key::Num6),
    ("7", Key::Num7),
    ("8", Key::Num8),
    ("9", Key::Num9),
];

impl Hotkey {
    /// Parse `Ctrl+Shift+F12`, `Pause`, `Alt+Win+S`... Without a modifier only
    /// keys nobody types in normal work are accepted (F-keys, Pause and the like).
    pub fn parse(s: &str) -> Option<Self> {
        let mut hk = Self { ctrl: false, alt: false, shift: false, win: false, key: Key::F12 };
        let mut key = None;
        for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => hk.ctrl = true,
                "alt" => hk.alt = true,
                "shift" => hk.shift = true,
                "win" | "meta" | "cmd" | "super" => hk.win = true,
                _ => {
                    let k = NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(part))?.1;
                    if key.replace(k).is_some() {
                        return None;
                    }
                }
            }
        }
        hk.key = key?;
        let modifier = hk.ctrl || hk.alt || hk.win;
        let special = !matches!(hk.key, Key::Space) && NAMED.iter().position(|(_, k)| *k == hk.key)? < 21;
        (modifier || special).then_some(hk)
    }

    pub fn matches(&self, key: Key, ctrl: bool, alt: bool, shift: bool, win: bool) -> bool {
        key == self.key && ctrl == self.ctrl && alt == self.alt && shift == self.shift && win == self.win
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [(self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.shift, "Shift"), (self.win, "Win")] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        let name = NAMED.iter().find(|(_, k)| *k == self.key).map_or("?", |(n, _)| n);
        write!(f, "{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints() {
        assert_eq!(Hotkey::parse("Ctrl+Shift+F12"), Some(Hotkey::default()));
        let p = Hotkey::parse("pause").unwrap();
        assert_eq!(p.key, Key::Pause);
        assert_eq!(Hotkey::parse("Alt+Win+s").unwrap().to_string(), "Alt+Win+S");
        // Plain letters or Shift+letter would fire while typing.
        assert_eq!(Hotkey::parse("S"), None);
        assert_eq!(Hotkey::parse("Shift+S"), None);
        assert_eq!(Hotkey::parse("Ctrl+S+T"), None);
        assert_eq!(Hotkey::parse("Ctrl+Banana"), None);
        assert!(Hotkey::default().matches(Key::F12, true, false, true, false));
        assert!(!Hotkey::default().matches(Key::F12, true, true, true, false));
    }
}
