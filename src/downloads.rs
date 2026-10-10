//! Downloads: Alloy refuses every download unless a handler says yes.
//! Files go to download-path (default ~/Downloads) and never overwrite.
//! Progress shows in a toast (ui::toast). Finished ones are kept in
//! ~/.local/share/rembrowser/downloads ("done|failed\tpath\turl", oldest first);
//! `:downloads` lists the running ones and that history.

use crate::{commands, popup, ui};
use cef::*;
use std::cell::RefCell;

/// A running download.
struct Dl {
    id: u32,
    name: String,
    path: String,
    url: String,
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

fn log_file() -> std::path::PathBuf {
    std::path::PathBuf::from(crate::data_dir()).join("downloads")
}

fn log_line(ok: bool, path: &str, url: &str) -> String {
    format!("{}\t{path}\t{url}", if ok { "done" } else { "failed" })
}

/// Finished downloads, oldest first: (ok, path, url).
fn parse_log(text: &str) -> Vec<(bool, String, String)> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut p = l.split('\t');
            let mut field = || p.next().unwrap_or("").to_string();
            (field() == "done", field(), field())
        })
        .collect()
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// `:downloads`: running downloads, then the history (newest first). Enter opens
/// the file; the row actions show its folder, forget the entry or delete the file.
pub fn picker() {
    let running: Vec<(String, String, i32)> = DLS.with_borrow(|d| {
        d.iter()
            .rev()
            .map(|x| (x.name.clone(), x.path.clone(), x.pct.max(0)))
            .collect()
    });
    let past = parse_log(&std::fs::read_to_string(log_file()).unwrap_or_default());
    let (r, n) = (running.len(), past.len());
    let mut rows: Vec<(&str, String, String)> = running
        .iter()
        .map(|(name, path, pct)| ("download", name.clone(), format!("{pct}%  {path}")))
        .collect();
    rows.extend(past.iter().rev().map(|(ok, path, _)| {
        let status = if *ok { "done" } else { "failed" };
        ("download", file_name(path), format!("{status}  {path}"))
    }));
    // Row i: a running download (i < r), else history entry n-1-(i-r).
    let entry = move |i: usize| (i >= r).then(|| n - 1 - (i - r));
    let open = past.clone();
    popup::pick_with(
        "\u{f019} Downloads",
        &rows,
        &[("C-o", "folder", "folder"), ("C-x", "forget", "forget"), ("C-d", "delete file", "delete")],
        move |i| match entry(i).map(|e| &open[e]) {
            None => ui::message("still downloading", false),
            Some((true, path, _)) if std::path::Path::new(path).exists() => {
                let _ = commands::command("xdg-open").arg(path).spawn();
            }
            Some((true, ..)) => ui::message("that file is gone", true),
            Some((false, ..)) => ui::message("that download failed", true),
        },
        move |act, i| {
            let Some(e) = entry(i) else {
                ui::message("still downloading", false);
                return;
            };
            let path = std::path::Path::new(&past[e].1);
            if act == "folder" {
                let _ = commands::command("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
                return;
            }
            if act == "delete" {
                let _ = std::fs::remove_file(path);
            }
            commands::edit_lines(&log_file(), |k, l| (k != e).then(|| l.to_string()));
            picker();
        },
    );
}

/// `:downloads-clear`: forget the list (the files stay).
pub fn clear() {
    let _ = std::fs::write(log_file(), "");
    ui::message("download list cleared", false);
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
                        url: CefString::from(&item.url()).to_string(),
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
            // A finished download leaves the running list for the history file.
            let dl = DLS.with_borrow_mut(|d| {
                let i = d.iter().position(|x| x.id == item.id())?;
                d[i].pct = item.percent_complete();
                d[i].speed = item.current_speed();
                d[i].state = state;
                Some(if state == State::Running { (d[i].name.clone(), None) } else { (d[i].name.clone(), Some(d.remove(i))) })
            });
            let Some((name, finished)) = dl else { return };
            if let Some(x) = finished {
                use std::io::Write;
                let _ = std::fs::create_dir_all(crate::data_dir());
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_file()) {
                    let _ = writeln!(f, "{}", log_line(state == State::Done, &x.path, &x.url));
                }
            }
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
    use super::{download_path, log_line, parse_log};

    #[test]
    fn download_history_round_trips() {
        let text = format!(
            "{}\n\n{}\n",
            log_line(true, "/home/u/Downloads/a b.pdf", "https://x.org/a%20b.pdf"),
            log_line(false, "/tmp/c.zip", "")
        );
        assert_eq!(
            parse_log(&text),
            vec![
                (true, "/home/u/Downloads/a b.pdf".into(), "https://x.org/a%20b.pdf".into()),
                (false, "/tmp/c.zip".into(), String::new()),
            ]
        );
    }

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
