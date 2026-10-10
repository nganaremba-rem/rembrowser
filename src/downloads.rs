//! Downloads: Alloy refuses every download unless a handler says yes.
//! Files go to download-path (default ~/Downloads) and never overwrite.
//! Progress shows in a toast (ui::toast); `:downloads` lists this session's.

use crate::{commands, popup, ui};
use cef::*;
use std::cell::RefCell;

struct Dl {
    id: u32,
    name: String,
    path: String,
    pct: i32,
    speed: i64,
    state: State,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Running,
    Done,
    Failed,
}

thread_local! {
    static DLS: RefCell<Vec<Dl>> = const { RefCell::new(Vec::new()) };
}

fn human(bytes: i64) -> String {
    let b = bytes as f64;
    match bytes {
        ..1_000 => format!("{bytes} B"),
        ..1_000_000 => format!("{:.0} kB", b / 1e3),
        ..1_000_000_000 => format!("{:.1} MB", b / 1e6),
        _ => format!("{:.2} GB", b / 1e9),
    }
}

/// Toast text: running downloads, else the one that just finished.
fn summary(name: &str, state: State) -> (String, bool) {
    DLS.with_borrow(|d| {
        let running: Vec<&Dl> = d.iter().filter(|x| x.state == State::Running).collect();
        match running.as_slice() {
            [] => match state {
                State::Done => (format!("✓  {name}  saved"), false),
                _ => (format!("✗  {name}  failed"), false),
            },
            [one] => (
                format!(
                    "↓  {}  {}%  · {}/s",
                    one.name,
                    one.pct.max(0),
                    human(one.speed)
                ),
                true,
            ),
            many => {
                let pct = many.iter().map(|x| x.pct.max(0)).sum::<i32>() / many.len() as i32;
                (format!("↓  {} downloads  {pct}%", many.len()), true)
            }
        }
    })
}

/// `:downloads`: this session's downloads; picking a finished one opens it.
pub fn picker() {
    let rows: Vec<(&str, String, String, String, bool)> = DLS.with_borrow(|d| {
        d.iter()
            .rev()
            .map(|x| {
                let status = match x.state {
                    State::Running => format!("{}%", x.pct.max(0)),
                    State::Done => "done".into(),
                    State::Failed => "failed".into(),
                };
                (
                    "download",
                    x.name.clone(),
                    format!("{status}  {}", x.path),
                    x.path.clone(),
                    x.state == State::Done,
                )
            })
            .collect()
    });
    let picks: Vec<(String, bool)> = rows.iter().map(|r| (r.3.clone(), r.4)).collect();
    let rows: Vec<_> = rows.into_iter().map(|(k, t, u, ..)| (k, t, u)).collect();
    popup::pick("\u{f019} Downloads", &rows, move |i| {
        let (path, done) = &picks[i];
        if *done {
            let _ = commands::command("xdg-open").arg(path).spawn();
        } else {
            ui::message("still downloading", false);
        }
    });
}

wrap_download_handler! {
    pub struct RemDownloadHandler;

    impl DownloadHandler {
        // Alloy refuses every download unless this says yes.
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> i32 {
            1
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> i32 {
            let Some(cb) = callback else { return 0 };
            let name = suggested_name.map(|n| n.to_string()).unwrap_or_default();
            let path = download_path(&name);
            if let Some(item) = download_item {
                DLS.with_borrow_mut(|d| {
                    d.push(Dl {
                        id: item.id(),
                        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                        path: path.to_string_lossy().into_owned(),
                        pct: 0,
                        speed: 0,
                        state: State::Running,
                    })
                });
            }
            cb.cont(Some(&path.to_string_lossy().as_ref().into()), 0);
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = download_item else { return };
            let state = if item.is_complete() != 0 {
                State::Done
            } else if item.is_canceled() != 0 || item.is_interrupted() != 0 {
                State::Failed
            } else {
                State::Running
            };
            let name = DLS.with_borrow_mut(|d| {
                let x = d.iter_mut().find(|x| x.id == item.id())?;
                x.pct = item.percent_complete();
                x.speed = item.current_speed();
                x.state = state;
                Some(x.name.clone())
            });
            let Some(name) = name else { return };
            let (text, running) = summary(&name, state);
            ui::toast(&text, running);
        }
    }
}

/// ~/Downloads/<name>, never overwriting: "file (1).pdf", "file (2).pdf", ...
fn download_path(name: &str) -> std::path::PathBuf {
    let dir = commands::opt("download-path")
        .map(|p| commands::expand_tilde(&p))
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Downloads")
        });
    let _ = std::fs::create_dir_all(&dir);
    // Strip path separators a hostile server could put in the suggested name.
    let name = match name
        .rsplit(['/', '\\'])
        .next()
        .filter(|n| !n.is_empty() && *n != "..")
    {
        Some(n) => n,
        None => "download",
    };
    let p = std::path::Path::new(name);
    let (stem, ext) = (
        p.file_stem().map_or(name.into(), |s| s.to_string_lossy()),
        p.extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default(),
    );
    let mut path = dir.join(name);
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::download_path;

    #[test]
    fn download_names_are_sanitized() {
        let p = download_path("../../etc/passwd");
        assert!(
            p.ends_with("Downloads/passwd")
                || p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("passwd (")
        );
        assert!(
            download_path("..").ends_with("Downloads/download")
                || download_path("..").to_string_lossy().contains("download (")
        );
    }
}
