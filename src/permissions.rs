//! Camera / mic / screen share / notifications / etc. Alloy has no built-in prompt, so
//! every request is refused unless we answer it: ask in the popup, remember
//! "always"/"never" per origin in ~/.local/share/rembrowser/permissions.

use crate::popup;
use cef::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

enum Pending {
    Media { cb: MediaAccessCallback, perms: u32 },
    Prompt { cb: PermissionPromptCallback },
}

struct Ask {
    origin: String,
    key: String,
    pending: Pending,
}

thread_local! {
    static NEXT_ID: RefCell<u64> = const { RefCell::new(1) };
    static ASKS: RefCell<HashMap<u64, Ask>> = RefCell::new(HashMap::new());
}

// CEF media bits (cef_media_access_permission_types_t).
const AUDIO: u32 = 1;
const VIDEO: u32 = 2;
const DESKTOP_AUDIO: u32 = 4;
const DESKTOP_VIDEO: u32 = 8;

/// Human words for a permission bitmask (cef_permission_request_types_t names).
fn describe(media: bool, bits: u32) -> String {
    let names: &[(u32, &str)] = if media {
        &[
            (AUDIO, "microphone"),
            (VIDEO, "camera"),
            (DESKTOP_VIDEO, "screen"),
            (DESKTOP_AUDIO, "system audio"),
        ]
    } else {
        &[
            (2, "camera controls"),
            (4, "camera"),
            (16, "clipboard"),
            (128, "local fonts"),
            (256, "location"),
            (4096, "microphone"),
            (16384, "multiple downloads"),
            (32768, "notifications"),
            (1048576, "cookies on other sites"),
            (8388608, "window placement"),
        ]
    };
    let words: Vec<&str> = names
        .iter()
        .filter(|(b, _)| bits & b != 0)
        .map(|(_, n)| *n)
        .collect();
    if words.is_empty() {
        format!("permission {bits:#x}")
    } else {
        words.join(" + ")
    }
}

fn store_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME").map_or_else(
        |_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share"),
        PathBuf::from,
    );
    base.join("rembrowser/permissions")
}

/// Stored decision for origin+permission: Some(true) always, Some(false) never.
fn stored(origin: &str, key: &str) -> Option<bool> {
    let text = std::fs::read_to_string(store_path()).ok()?;
    text.lines().rev().find_map(|l| {
        let mut p = l.split('\t');
        (p.next() == Some(origin) && p.next() == Some(key)).then(|| p.next() == Some("allow"))
    })
}

/// Every remembered decision, latest answer per (origin, permission):
/// (origin, key, allow, what it is in words).
pub fn remembered() -> Vec<(String, String, bool, String)> {
    let text = std::fs::read_to_string(store_path()).unwrap_or_default();
    let mut out: Vec<(String, String, bool, String)> = Vec::new();
    for l in text.lines() {
        let mut p = l.split('\t');
        let (Some(origin), Some(key), Some(verdict)) = (p.next(), p.next(), p.next()) else {
            continue;
        };
        let what = key
            .split_once(':')
            .and_then(|(kind, bits)| Some(describe(kind == "media", bits.parse().ok()?)))
            .unwrap_or_else(|| key.to_string());
        out.retain(|x| !(x.0 == origin && x.1 == key));
        out.push((origin.into(), key.into(), verdict == "allow", what));
    }
    out
}

/// Drop a remembered decision: the site gets asked again.
pub fn forget(origin: &str, key: &str) {
    crate::commands::edit_lines(&store_path(), |_, l| {
        let mut p = l.split('\t');
        (!(p.next() == Some(origin) && p.next() == Some(key))).then(|| l.to_string())
    });
}

fn remember(origin: &str, key: &str, allow: bool) {
    use std::io::Write;
    let path = store_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            f,
            "{origin}\t{key}\t{}",
            if allow { "allow" } else { "deny" }
        );
    }
}

fn finish(p: Pending, allow: bool) {
    match p {
        Pending::Media { cb, perms } => {
            if allow {
                cb.cont(perms);
            } else {
                cb.cancel();
            }
        }
        Pending::Prompt { cb } => cb.cont(if allow {
            PermissionRequestResult::ACCEPT
        } else {
            PermissionRequestResult::DENY
        }),
    }
}

fn request(browser: Option<&Browser>, origin: &str, media: bool, bits: u32, pending: Pending) {
    let key = format!("{}:{bits}", if media { "media" } else { "perm" });
    if let Some(allow) = stored(origin, &key) {
        finish(pending, allow);
        return;
    }
    let id = NEXT_ID.with_borrow_mut(|n| {
        *n += 1;
        *n
    });
    let what = describe(media, bits);
    ASKS.with_borrow_mut(|a| {
        a.insert(
            id,
            Ask {
                origin: origin.into(),
                key,
                pending,
            },
        )
    });
    // Always the popup of the asking page's window: the popup is our own browser,
    // which the page cannot reach. (A prompt drawn inside the page could be answered
    // by the page itself.)
    popup::ask(id, origin, &what, browser.and_then(crate::tabs::owner_of));
}

/// From the popup: allow | always | deny | never | dismiss.
pub fn answer(id: u64, choice: &str) {
    let Some(ask) = ASKS.with_borrow_mut(|a| a.remove(&id)) else {
        return;
    };
    let allow = matches!(choice, "allow" | "always");
    if matches!(choice, "always" | "never") {
        remember(&ask.origin, &ask.key, allow);
    }
    finish(ask.pending, allow);
}

fn origin_of(o: Option<&CefString>) -> String {
    o.map(|s| s.to_string())
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
}

wrap_permission_handler! {
    pub struct RemPermissionHandler;

    impl PermissionHandler {
        // getUserMedia / getDisplayMedia (calls, screen share).
        fn on_request_media_access_permission(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> i32 {
            let Some(cb) = callback else { return 0 };
            let pending = Pending::Media { cb: cb.clone(), perms: requested_permissions };
            request(browser.as_deref(), &origin_of(requesting_origin), true, requested_permissions, pending);
            1
        }

        // Everything else Chromium would show a prompt bubble for (notifications, ...).
        fn on_show_permission_prompt(
            &self,
            browser: Option<&mut Browser>,
            _prompt_id: u64,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> i32 {
            let Some(cb) = callback else { return 0 };
            // geolocation=never: refuse location without asking.
            if requested_permissions == 256 && crate::commands::opt("geolocation").as_deref() == Some("never") {
                cb.cont(PermissionRequestResult::DENY);
                return 1;
            }
            request(browser.as_deref(), &origin_of(requesting_origin), false, requested_permissions, Pending::Prompt { cb: cb.clone() });
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::describe;

    #[test]
    fn describes_permissions() {
        assert_eq!(describe(true, 3), "microphone + camera");
        assert_eq!(describe(false, 32768), "notifications");
        assert_eq!(describe(false, 1 << 30), "permission 0x40000000");
    }
}
