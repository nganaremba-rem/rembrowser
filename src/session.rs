//! Session restore: the open tabs of every window, saved to `<data>/session` as they
//! change, and a clean-exit marker so a start after a crash / kill can bring them back.
//!
//! File format (plain text, one URL per line):
//!   window<TAB><active tab index>
//!   https://example.org/
//!   ...

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, PartialEq)]
pub struct Win {
    pub urls: Vec<String>,
    pub active: usize,
}

static CRASHED: AtomicBool = AtomicBool::new(false);

fn dir() -> PathBuf {
    PathBuf::from(crate::data_dir())
}

fn session_file() -> PathBuf {
    dir().join("session")
}

fn marker() -> PathBuf {
    dir().join("clean-exit")
}

/// At startup: was the last run ended cleanly? (No marker = crash, kill, power loss.)
/// The marker is removed now and written again by `mark_clean` on a normal quit.
pub fn start() {
    CRASHED.store(!marker().exists(), Ordering::Relaxed);
    let _ = std::fs::remove_file(marker());
}

pub fn crashed() -> bool {
    CRASHED.load(Ordering::Relaxed)
}

pub fn mark_clean() {
    let _ = std::fs::write(marker(), b"");
}

pub fn format(wins: &[Win]) -> String {
    let mut out = String::new();
    for w in wins {
        out.push_str(&format!("window\t{}\n", w.active));
        for u in &w.urls {
            out.push_str(u);
            out.push('\n');
        }
    }
    out
}

pub fn parse(text: &str) -> Vec<Win> {
    let mut wins: Vec<Win> = Vec::new();
    for line in text.lines() {
        if let Some(a) = line.strip_prefix("window\t") {
            wins.push(Win { urls: Vec::new(), active: a.parse().unwrap_or(0) });
        } else if !line.is_empty() {
            if let Some(w) = wins.last_mut() {
                w.urls.push(line.to_string());
            }
        }
    }
    wins.retain(|w| !w.urls.is_empty());
    for w in &mut wins {
        w.active = w.active.min(w.urls.len() - 1);
    }
    wins
}

/// Atomic: write a temp file, then rename over the old one, so a crash mid-write
/// never leaves a half session.
pub fn save(wins: &[Win]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = dir().join("session.tmp");
    let ok = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(format(wins).as_bytes()))
        .is_ok();
    if ok {
        let _ = std::fs::rename(&tmp, session_file());
    }
}

pub fn load() -> Vec<Win> {
    parse(&std::fs::read_to_string(session_file()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trips() {
        let wins = vec![
            Win { urls: vec!["https://a.org/".into(), "https://b.org/x?y=1".into()], active: 1 },
            Win { urls: vec!["https://c.org/".into()], active: 0 },
        ];
        assert_eq!(parse(&format(&wins)), wins);
    }

    #[test]
    fn parse_ignores_junk_and_clamps_active() {
        let text = "https://orphan.example/\nwindow\t9\nhttps://a.org/\n\nwindow\t0\n";
        let wins = parse(text);
        assert_eq!(wins, vec![Win { urls: vec!["https://a.org/".into()], active: 0 }]);
    }
}
