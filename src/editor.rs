//! External editor (nvim popups): form fields (insert-mode Ctrl-T, `;e`) and the
//! command line (popup Ctrl-F). Text goes through a temp file; nothing changes
//! unless the editor wrote the file.

use crate::{commands, ui};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// Runs `cmd` (a shell command, `%s` = the file) on a worker thread; `done` gets the
/// new text on the UI thread, only if the file was saved.
pub fn edit(text: &str, cmd: &str, done: impl FnOnce(String) + Send + 'static) {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("rembrowser-{}-{n}.txt", std::process::id()));
    if std::fs::write(&path, text).is_err() {
        ui::message("can't write temp file", true);
        return;
    }
    let before = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let quoted = format!("'{}'", path.display().to_string().replace('\'', r"'\''"));
    let cmd = if cmd.contains("%s") {
        cmd.replace("%s", &quoted)
    } else {
        format!("{cmd} {quoted}")
    };
    let mut c = commands::command("sh");
    c.arg("-c").arg(cmd);
    std::thread::spawn(move || {
        let status = c.status();
        let ok = status.as_ref().is_ok_and(|s| s.success());
        if !ok {
            ui::run_on_ui(0, || {
                ui::message("editor failed — check `set editor-command=... %s`", true)
            });
        }
        let saved = std::fs::metadata(&path).and_then(|m| m.modified()).ok() != before;
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        if ok && saved {
            ui::run_on_ui(0, move || done(text));
        }
    });
}

/// The field editor: `editor-command` (`%s` = file).
pub fn field_command() -> String {
    commands::opt("editor-command").unwrap_or_else(|| {
        "foot --app-id rembrowser-editor env NVIM_APPNAME=nvim-rembrowser REMBROWSER_POPUP=field nvim %s".into()
    })
}

/// decodeURIComponent (page.js sends field text percent-encoded: tabs/newlines survive).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).ok()?, 16).ok())
            .flatten();
        match hex {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::percent_decode;

    #[test]
    fn decodes_uri_components() {
        assert_eq!(percent_decode("a%20b%0Ac%09d"), "a b\nc\td");
        assert_eq!(percent_decode("%C3%A9t%C3%A9"), "été");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
    }
}
