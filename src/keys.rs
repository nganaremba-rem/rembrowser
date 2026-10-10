//! Vim key notation <-> CEF key events.
//!
//! A key is a string token: a printable char ("j", "G", "$") or a bracketed name
//! ("<C-d>", "<Space>", "<CR>", "<S-Tab>"). Ctrl letters are lowercase, so
//! `<C-D>` in a config means the same as `<C-d>` (vim treats them alike).

use cef::KeyEvent;

pub type Key = String;

const SHIFT: u32 = 1 << 1;
const CTRL: u32 = 1 << 2;
const ALT: u32 = 1 << 3;

fn special_name(code: i32) -> Option<&'static str> {
    Some(match code {
        8 => "BS",
        9 => "Tab",
        13 => "CR",
        27 => "Esc",
        32 => "Space",
        33 => "PageUp",
        34 => "PageDown",
        35 => "End",
        36 => "Home",
        37 => "Left",
        38 => "Up",
        39 => "Right",
        40 => "Down",
        46 => "Del",
        112..=123 => [
            "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
        ][(code - 112) as usize],
        _ => return None,
    })
}

/// Key token for a RAWKEYDOWN event. None for bare modifiers and unknown keys.
pub fn from_event(e: &KeyEvent) -> Option<Key> {
    let (ctrl, alt, shift) = (
        e.modifiers & CTRL != 0,
        e.modifiers & ALT != 0,
        e.modifiers & SHIFT != 0,
    );
    let code = e.windows_key_code;
    // The typed character wins over the keycode: remappers (keyd, kanata, wtype) can put
    // ':' on a keycode Chromium names Escape, but `character` is still ':'.
    if !ctrl && !alt {
        match char::from_u32(e.character as u32) {
            Some(' ') => return Some("<Space>".into()),
            Some(c) if !c.is_control() => return Some(c.to_string()),
            _ => {}
        }
    }
    let mut prefix = String::new();
    if ctrl {
        prefix.push_str("C-");
    }
    if alt {
        prefix.push_str("M-");
    }
    // Same rule with Ctrl/Alt: the key's character (Ctrl+= is "=") beats the keycode,
    // which remappers and wtype reassign (Ctrl+= arrived as keycode "Escape").
    if ctrl || alt {
        if let Some(c) =
            char::from_u32(e.unmodified_character as u32).filter(|c| !c.is_control() && *c != ' ')
        {
            return Some(format!("<{prefix}{}>", c.to_ascii_lowercase()));
        }
    }
    if let Some(name) = special_name(code) {
        // Shift only matters on non-printables (<S-Tab>); on letters it's already in the char.
        let s = if shift { "S-" } else { "" };
        return Some(format!("<{prefix}{s}{name}>"));
    }
    if ctrl || alt {
        // The char is a control code here (Ctrl-D = \x04): name the key by its keycode.
        let c = match code {
            65..=90 => (code as u8 + 32) as char,
            48..=57 => code as u8 as char,
            187 => '=', // VK_OEM_PLUS
            189 => '-', // VK_OEM_MINUS
            _ => char::from_u32(e.unmodified_character as u32).filter(|c| !c.is_control())?,
        };
        return Some(format!("<{prefix}{c}>"));
    }
    char::from_u32(e.character as u32)
        .filter(|c| !c.is_control())
        .map(String::from)
}

/// "gT<C-d><Space>x" -> ["g", "T", "<C-d>", "<Space>", "x"]. Unknown <...> stays literal chars.
pub fn parse(s: &str) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            if let Some(end) = rest.find('>') {
                if let Some(k) = normalize(&rest[1..end]) {
                    keys.push(k);
                    rest = &rest[end + 1..];
                    continue;
                }
            }
        }
        keys.push(c.to_string());
        rest = &rest[c.len_utf8()..];
    }
    keys
}

/// "C-D" -> "<C-d>", "space" -> "<Space>", "lt" -> "<". None if not a key name.
fn normalize(inner: &str) -> Option<Key> {
    let mut mods = String::new();
    let mut name = inner;
    while name.len() > 2 && name.as_bytes()[1] == b'-' {
        match name.as_bytes()[0].to_ascii_uppercase() {
            b'C' => mods.push_str("C-"),
            b'M' | b'A' => mods.push_str("M-"),
            b'S' => mods.push_str("S-"),
            _ => return None,
        }
        name = &name[2..];
    }
    let lower = name.to_ascii_lowercase();
    let canonical = match lower.as_str() {
        "lt" if mods.is_empty() => return Some("<".into()),
        "bar" if mods.is_empty() => return Some("|".into()),
        "space" => "Space",
        "cr" | "enter" | "return" => "CR",
        "esc" => "Esc",
        "tab" => "Tab",
        "bs" | "backspace" => "BS",
        "del" => "Del",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        f if f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => {
            return Some(format!("<{mods}F{}>", &f[1..]));
        }
        _ if name.chars().count() == 1 && !mods.is_empty() => {
            let c = name.chars().next()?;
            // <C-D> == <C-d>; <S-x> is just "X".
            if mods == "S-" {
                return Some(c.to_uppercase().to_string());
            }
            let c = if mods.contains("C-") || mods.contains("M-") {
                c.to_ascii_lowercase()
            } else {
                c
            };
            return Some(format!("<{}{c}>", mods.replace("S-", "")));
        }
        _ => return None,
    };
    Some(format!("<{mods}{canonical}>"))
}

/// Printable text a key inserts in the command line (None for named keys).
pub fn text(k: &str) -> Option<&str> {
    match k {
        "<Space>" => Some(" "),
        _ if !k.starts_with('<') || k == "<" => Some(k),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: i32, ch: u16, unmod: u16, mods: u32) -> KeyEvent {
        KeyEvent {
            size: std::mem::size_of::<KeyEvent>(),
            type_: cef::KeyEventType::RAWKEYDOWN,
            modifiers: mods,
            windows_key_code: code,
            native_key_code: 0,
            is_system_key: 0,
            character: ch,
            unmodified_character: unmod,
            focus_on_editable_field: 0,
        }
    }

    #[test]
    fn ctrl_keys_are_named_by_character() {
        // A real keyboard: Ctrl+= / Ctrl+- / Ctrl+0 / Ctrl+D / Ctrl+Tab / Esc.
        assert_eq!(
            from_event(&ev(187, 0x3d, 0x3d, CTRL)).as_deref(),
            Some("<C-=>")
        );
        assert_eq!(
            from_event(&ev(189, 0x1f, 0x2d, CTRL)).as_deref(),
            Some("<C-->")
        );
        assert_eq!(
            from_event(&ev(48, 0x30, 0x30, CTRL)).as_deref(),
            Some("<C-0>")
        );
        assert_eq!(
            from_event(&ev(68, 0x04, 0x64, CTRL)).as_deref(),
            Some("<C-d>")
        );
        assert_eq!(
            from_event(&ev(9, 0x09, 0x09, CTRL)).as_deref(),
            Some("<C-Tab>")
        );
        assert_eq!(from_event(&ev(27, 0x1b, 0x1b, 0)).as_deref(), Some("<Esc>"));
        // Character known only through the keycode (control char): still <C-=>.
        assert_eq!(from_event(&ev(187, 0, 0, CTRL)).as_deref(), Some("<C-=>"));
    }

    #[test]
    fn parses_vim_notation() {
        assert_eq!(parse("gT"), ["g", "T"]);
        assert_eq!(parse("<C-D><c-u>"), ["<C-d>", "<C-u>"]);
        assert_eq!(parse("<Space>sb"), ["<Space>", "s", "b"]);
        assert_eq!(parse(":q<CR>"), [":", "q", "<CR>"]);
        assert_eq!(parse("<S-Tab><lt>x"), ["<S-Tab>", "<", "x"]);
        assert_eq!(parse("a<b"), ["a", "<", "b"]);
        assert_eq!(parse("<C-R>%"), ["<C-r>", "%"]);
        assert_eq!(parse("<F5>"), ["<F5>"]);
    }

    #[test]
    fn text_of_keys() {
        assert_eq!(text("<Space>"), Some(" "));
        assert_eq!(text("x"), Some("x"));
        assert_eq!(text("<CR>"), None);
    }
}
