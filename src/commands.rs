//! Ex commands + options. The config file is just ex commands, so it
//! runs through the same `exec` as the `:` command line.

use crate::{pickers, tabs, ui, vim};
use cef::ImplBrowser;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Default)]
struct Opts {
    set: HashMap<String, String>,
    shortcuts: HashMap<String, String>,
    default_shortcut: Option<String>,
}

thread_local! {
    static OPTS: RefCell<Opts> = RefCell::new(Opts::default());
}

pub fn opt(key: &str) -> Option<String> {
    OPTS.with_borrow(|o| o.set.get(key).cloned())
}

pub fn opt_bool(key: &str, default: bool) -> bool {
    opt(key).map_or(default, |v| {
        matches!(v.as_str(), "on" | "true" | "1" | "yes")
    })
}

pub fn opt_num(key: &str, default: i32) -> i32 {
    opt(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

pub fn expand_tilde(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => PathBuf::from(home()).join(rest),
        None => PathBuf::from(p),
    }
}

/// ~/.local/share/rembrowser: history, bookmark, harpoon and queue live here.
pub fn data_dir() -> PathBuf {
    PathBuf::from(crate::data_dir())
}

/// Built-in defaults, then config.yaml, else the older ~/.config/rembrowser/config.
pub fn load_config() {
    let cfg = std::env::var("XDG_CONFIG_HOME")
        .map_or_else(|_| PathBuf::from(home()).join(".config"), PathBuf::from);
    let path = cfg.join("rembrowser/config");
    source_text(include_str!("default.config"));
    // config.yaml wins over the older ex-command file.
    if crate::config::path().exists() {
        if let Err(e) = crate::config::load() {
            eprintln!("rembrowser: {e}");
        }
    } else if path.exists() {
        source(&path);
    }
}

/// One command, no `|` splitting (config.yaml values may contain `|`).
pub fn exec_one_line(line: &str) {
    exec_one(line);
}

pub fn map_with_desc(cmd: &str, lhs: &str, rhs: &str, desc: Option<String>) {
    vim::map(&cmd[..1], lhs, rhs, cmd.contains("noremap"), desc);
}

fn source(path: &std::path::Path) {
    match std::fs::read_to_string(path) {
        Ok(text) => source_text(&text),
        // Startup config runs before CEF exists: no UI to show it in yet.
        Err(_) if !ui::is_up() => eprintln!("rembrowser: can't read {}", path.display()),
        Err(_) => ui::message(&format!("can't read {}", path.display()), true),
    }
}

fn source_text(text: &str) {
    // `#:` above a map = its description in the :keys cheatsheet.
    let mut desc = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(d) = line.strip_prefix("#:") {
            desc = Some(d.trim().to_string());
        } else if line.is_empty() || line.starts_with('#') || line.starts_with('"') {
            desc = None;
        } else {
            MAP_DESC.set(desc.take());
            exec(line);
        }
    }
    MAP_DESC.set(None);
}

thread_local! {
    static MAP_DESC: std::cell::Cell<Option<String>> = const { std::cell::Cell::new(None) };
}

const MAP_CMDS: &[&str] = &[
    "nmap", "nnoremap", "imap", "inoremap", "cmap", "cnoremap", "nunmap", "iunmap", "cunmap",
];

/// Run one command line. `a|b` runs both, except for map commands whose RHS owns the `|`
/// (`nnoremap <Space>uz :set status-bar!|set fullscreen!<CR>`).
pub fn exec(line: &str) {
    let line = line.trim().trim_start_matches(':').trim();
    let first = line.split_whitespace().next().unwrap_or("");
    if MAP_CMDS.contains(&first) {
        exec_one(line);
    } else {
        for part in line.split('|') {
            exec_one(part);
        }
    }
}

fn exec_one(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let name_end = line
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or(line.len());
    let (name, rest) = line.split_at(name_end);
    let bang = rest.starts_with('!');
    let args = rest.trim_start_matches('!').trim();
    match name {
        "o" | "open" => open(args, false),
        "t" | "tabopen" | "tabnew" => open(args, true),
        "winopen" | "window" => tabs::create_window(resolve_url(args)),
        "q" | "quit" | "close" | "tabclose" => tabs::close_active(),
        "qa" | "qall" | "quitall" | "wqa" => tabs::quit_all(),
        "back" => tabs::with_active(|b| b.go_back()),
        "forward" => tabs::with_active(|b| b.go_forward()),
        "reload" if bang => tabs::with_active(|b| b.reload_ignore_cache()),
        "reload" => tabs::with_active(|b| b.reload()),
        "stop" => tabs::with_active(|b| b.stop_load()),
        "tabnext" | "tabn" => tabs::cycle(1),
        "tabprev" | "tabp" | "tabprevious" => tabs::cycle(-1),
        "tabfirst" => tabs::select(0),
        "tablast" => tabs::select(usize::MAX),
        "b" | "buffer" => match args.parse::<usize>() {
            Ok(n) if n > 0 => tabs::select(n - 1),
            _ => ui::message("usage: :buffer N", true),
        },
        "tabs" | "ls" => pickers::tabs(),
        "history" | "hist" => pickers::history(),
        "bookmarks" | "bmarks" => pickers::bookmarks(),
        "pins" => pickers::pins(),
        "pin" => pickers::pin(args),
        "jump" => pickers::jump(args),
        "keys" | "map" => pickers::keys(),
        "downloads" | "dl" => crate::downloads::picker(),
        "downloads-clear" => crate::downloads::clear(),
        "queue" => pickers::queue(),
        "history-clear" => pickers::history_clear(),
        "permissions" => pickers::permissions(),
        "pip" => tabs::pip(),
        "mpv" => mpv(args),
        "config" => crate::config::open_in_editor(false),
        // Chromium's own pages, in a window with Chromium's UI layer.
        "settings" => tabs::chrome_window("chrome://settings", false),
        "private" => private_window(args),
        "extensions" | "ext-popup" => crate::extensions::picker(),
        "ext-manage" => tabs::chrome_window("chrome://extensions", true),
        "ext-install" => crate::extensions::install(args),
        "ext-remove" => crate::extensions::remove_picker(),
        "chrome" if args.is_empty() => tabs::chrome_window(&tabs::active_url(), true),
        "chrome" => tabs::chrome_window(&resolve_url(args), true),
        // :app [URL] — this page (or URL) as a web app; the tab closes.
        "app" if args.is_empty() => {
            let url = tabs::active_url();
            crate::apps::open(&url);
            tabs::close_active_now();
        }
        "app" => crate::apps::open(args),
        // :app-install [name] — launcher entry that opens this page as a web app.
        "app-install" => {
            let url = tabs::active_url();
            let name = if args.is_empty() {
                tabs::active_title()
            } else {
                args.to_string()
            };
            match crate::apps::install(&name, &url, tabs::active_favicon()) {
                Ok(f) => ui::message(
                    &format!("added \"{name}\" to your app launcher ({f})"),
                    false,
                ),
                Err(e) => ui::message(&format!("app-install: {e}"), true),
            }
        }
        "userscripts" => ui::message(
            &format!(
                "{} userscript(s) from ~/.config/rembrowser/userscripts",
                crate::userscripts::load()
            ),
            false,
        ),
        "set" => set(args),
        "nmap" | "nnoremap" | "imap" | "inoremap" | "cmap" | "cnoremap" => map(name, args),
        "nunmap" | "iunmap" | "cunmap" => vim::unmap(&name[..1], args),
        "shortcut-add" => match args.split_once('=') {
            Some((k, v)) => {
                OPTS.with_borrow_mut(|o| o.shortcuts.insert(k.trim().into(), v.trim().into()));
            }
            None => ui::message("usage: shortcut-add name=url$0", true),
        },
        "shortcut-default" => OPTS.with_borrow_mut(|o| o.default_shortcut = Some(args.into())),
        "source" | "so" => source(&expand_tilde(args)),
        "sh" | "shell" => shell(args, !bang),
        "shellex" => shellex(args),
        "normal" | "norm" => vim::run_keys(args, bang),
        "eval" | "e" => tabs::js(args),
        "bma" => bookmark_add(args),
        "qpush" => queue_push(),
        "qpop" => queue_pop(),
        "echo" => ui::message(args, false),
        "zoom" => vim::zoom(args.parse().unwrap_or(0.0)),
        _ => ui::message(&format!("Not a command: {name}"), true),
    }
}

fn map(cmd: &str, args: &str) {
    let Some((lhs, rhs)) = args.split_once(char::is_whitespace) else {
        ui::message(&format!("usage: {cmd} LHS RHS"), true);
        return;
    };
    vim::map(
        &cmd[..1],
        lhs,
        rhs.trim_start(),
        cmd.contains("noremap"),
        MAP_DESC.take(),
    );
}

/// `set k=v`, `set k!` (toggle), `set k` (on), `set nok` (off).
fn set(args: &str) {
    let (key, val) = match args.split_once('=') {
        Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
        None if args.ends_with('!') => {
            let k = args.trim_end_matches('!').trim().to_string();
            let v = if opt_bool(&k, default_on(&k)) {
                "off"
            } else {
                "on"
            };
            (k, v.into())
        }
        None => match args.strip_prefix("no") {
            Some(k) if opt(args).is_none() => (k.into(), "off".into()),
            _ => (args.into(), "on".into()),
        },
    };
    OPTS.with_borrow_mut(|o| o.set.insert(key.clone(), val.clone()));
    match key.as_str() {
        "status-bar" | "tab-bar" | "bookmark-bar" => ui::bars_changed(),
        // Read once at startup (they decide what gets loaded into the process).
        "adblock" | "drm" | "extensions" | "dark-mode" if ui::is_up() => {
            ui::message(&format!("{key} = {val}: applies after a restart"), false)
        }
        "fullscreen" => tabs::set_fullscreen(opt_bool("fullscreen", false)),
        _ => {}
    }
}

fn default_on(key: &str) -> bool {
    matches!(
        key,
        "smooth-scrolling" | "incsearch" | "confirm-quit" | "adblock" | "drm"
    )
}

/// Options rembrowser acts on, with their current value (for :set completion).
pub fn settings() -> Vec<(String, String)> {
    const KNOWN: &[&str] = &[
        "adblock",
        "bookmark-bar",
        "confirm-quit",
        "dark-mode",
        "drm",
        "editor-command",
        "extensions",
        "fullscreen",
        "geolocation",
        "hint-keys",
        "home-page",
        "incsearch",
        "mpv-command",
        "scroll-step",
        "smooth-scrolling",
        "status-bar",
        "tab-bar",
        "x-hint-command",
    ];
    KNOWN
        .iter()
        .map(|k| {
            let v = opt(k).unwrap_or_else(|| {
                if default_on(k) {
                    "on".into()
                } else {
                    String::new()
                }
            });
            (k.to_string(), v)
        })
        .collect()
}

/// Text from `:open` → URL. Shortcuts: `aw niri` → arch wiki search; plain words →
/// default shortcut; things that look like hosts get https://.
pub fn resolve_url(arg: &str) -> String {
    let arg = arg.trim();
    if arg.is_empty() {
        return crate::home::home_page();
    }
    if arg.contains("://")
        || ["about:", "data:", "file:", "chrome:", "view-source:"]
            .iter()
            .any(|p| arg.starts_with(p))
    {
        return arg.into();
    }
    let (first, rest) = arg.split_once(' ').unwrap_or((arg, ""));
    let template = OPTS.with_borrow(|o| o.shortcuts.get(first).cloned());
    if let Some(t) = template.filter(|_| !rest.is_empty()) {
        return fill(&t, rest);
    }
    if !arg.contains(' ') && looks_like_host(arg) {
        let local =
            arg.starts_with("localhost") || arg.starts_with("127.") || arg.starts_with("192.168.");
        return format!("{}://{arg}", if local { "http" } else { "https" });
    }
    let default = OPTS.with_borrow(|o| {
        o.default_shortcut
            .as_ref()
            .and_then(|d| o.shortcuts.get(d))
            .cloned()
    });
    fill(
        &default.unwrap_or_else(|| "https://duckduckgo.com/?q=$0".into()),
        arg,
    )
}

fn looks_like_host(s: &str) -> bool {
    let host = s.split(['/', '?', '#']).next().unwrap_or("");
    let host = host
        .rsplit_once(':')
        .filter(|(_, p)| p.parse::<u16>().is_ok())
        .map_or(host, |(h, _)| h);
    host == "localhost"
        || (host.contains('.')
            && !host.starts_with('.')
            && !host.ends_with('.')
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
}

/// Shortcut template: $0 = all words, $1.. = single words.
fn fill(template: &str, words: &str) -> String {
    let parts: Vec<&str> = words.split_whitespace().collect();
    let mut out = template.replace("$0", &encode(words));
    for (i, p) in parts.iter().enumerate().rev() {
        out = out.replace(&format!("${}", i + 1), &encode(p));
    }
    out
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn open(arg: &str, new_tab: bool) {
    let url = resolve_url(arg);
    if tabs::needs_chrome(&url) {
        tabs::chrome_window(&url, true);
    } else if new_tab {
        tabs::open(&url, false);
    } else {
        tabs::load(&url);
    }
}

/// Every program rembrowser starts goes through here. Chromium's sockets/files aren't
/// close-on-exec, so a long-lived child (wl-copy, mpv) would otherwise keep them open,
/// e.g. hold the DevTools port after the browser quit. Mark fds > 2 CLOEXEC just
/// before exec (not close: std still needs its own error-report pipe until exec).
pub fn command(program: &str) -> Command {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new(program);
    // SAFETY: close_range is a single async-signal-safe syscall.
    unsafe {
        c.pre_exec(|| {
            libc::close_range(
                3,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC as libc::c_int,
            );
            Ok(())
        });
    }
    c
}

/// `:private [URL]`: a private window: its own throwaway profile, so no history, cookies
/// or logins carry in or out. It is another rembrowser process (`--private`).
fn private_window(url: &str) {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = command(&exe.to_string_lossy());
    cmd.arg("--private");
    if !url.is_empty() {
        cmd.arg(resolve_url(url));
    }
    match cmd.spawn() {
        // Reap it when that window quits.
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => ui::message(&format!("private: {e}"), true),
    }
}

/// `;m` / `:mpv [URL]`: play in mpv (which uses yt-dlp for YouTube & co.). No URL, or
/// a <video> whose source is a blob: this page.
pub fn mpv(url: &str) {
    let url = if url.is_empty() || url.starts_with("blob:") {
        tabs::active_url()
    } else {
        url.to_string()
    };
    let cmd = opt("mpv-command").unwrap_or_else(|| "mpv".into());
    let mut words = cmd.split_whitespace();
    let prog = words.next().unwrap_or("mpv");
    let spawned = command(prog)
        .args(words)
        .arg("--")
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(_) => ui::message(&format!("mpv \u{25b6} {url}"), false),
        Err(e) => ui::message(&format!("{prog}: {e}"), true),
    }
}

fn env_cmd(cmd: &str) -> Command {
    let (uri, title) = (tabs::active_url(), tabs::active_title());
    let mut c = command("sh");
    c.arg("-c").arg(cmd).stdin(Stdio::null());
    c.env("REMBROWSER_URI", &uri)
        .env("REMBROWSER_TITLE", &title)
        .env("REMBROWSER_PID", std::process::id().to_string());
    c
}

/// `:sh! cmd` = fire and forget; `:sh cmd` = also show the first output line.
fn shell(cmd: &str, show: bool) {
    let mut c = env_cmd(cmd);
    if !show {
        let _ = c.stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        return;
    }
    std::thread::spawn(move || {
        let out = c.output().map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_string()
        });
        ui::run_on_ui(0, move || {
            ui::message(&out.unwrap_or_else(|e| e.to_string()), false)
        });
    });
}

/// `:shellex cmd` runs cmd and executes each stdout line as an ex command
/// (a picker script prints `open URL`, `normal KEYS`).
fn shellex(cmd: &str) {
    let mut c = env_cmd(cmd);
    c.stderr(Stdio::null());
    std::thread::spawn(move || {
        let Ok(out) = c.output() else { return };
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        ui::run_on_ui(0, move || text.lines().for_each(exec));
    });
}

fn append_line(file: &str, line: &str) {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(file))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Rewrite a line file: `keep` gets (index among the non-blank lines, line) and
/// returns the line to store (None drops it). Temp file + rename, so a crash can't
/// leave half a file.
pub fn edit_lines(path: &std::path::Path, mut keep: impl FnMut(usize, &str) -> Option<String>) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let out: String = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .filter_map(|(i, l)| keep(i, l))
        .map(|l| l + "\n")
        .collect();
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, out).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

/// History file: "uri\ttitle", oldest first.
pub fn record_history(url: &str, title: &str) {
    if url.starts_with("http") && !crate::home::is_home(url) {
        append_line("history", &format!("{url}\t{}", title.replace('\t', " ")));
    }
}

/// (url, title, tags) of every bookmark, in file order.
pub fn bookmarks() -> Vec<(String, String, String)> {
    // Entry n is the n-th non-blank line: the index edit_lines() works with.
    std::fs::read_to_string(data_dir().join("bookmark"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut p = l.split('\t');
            let mut field = || p.next().unwrap_or("").to_string();
            (field(), field(), field())
        })
        .collect()
}

fn bookmark_add(tags: &str) {
    let url = tabs::active_url();
    append_line(
        "bookmark",
        &format!("{url}\t{}\t{tags}", tabs::active_title().replace('\t', " ")),
    );
    ui::refresh_bookmarks();
    ui::message(&format!("bookmarked {url}"), false);
}

fn queue_push() {
    queue_add(&tabs::active_url(), false);
}

/// Read-later queue: `;p` appends, `;P` puts it first.
pub fn queue_add(url: &str, front: bool) {
    if url.is_empty() {
        return;
    }
    if front {
        let path = data_dir().join("queue");
        let rest = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(&path, format!("{url}\n{rest}"));
    } else {
        append_line("queue", url);
    }
    ui::message(&format!("queued {url}"), false);
}

fn queue_pop() {
    let path = data_dir().join("queue");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines = text.lines();
    match lines.next() {
        Some(first) => {
            let rest: Vec<&str> = lines.collect();
            let _ = std::fs::write(
                &path,
                if rest.is_empty() {
                    String::new()
                } else {
                    rest.join("\n") + "\n"
                },
            );
            open(first, false);
        }
        None => ui::message("read-later queue is empty", true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_urls() {
        exec("shortcut-add dd=https://duckduckgo.com/?q=$0");
        exec("shortcut-add aw=https://wiki.archlinux.org/index.php?search=$0");
        exec("shortcut-default dd");
        assert_eq!(
            resolve_url("aw niri wm"),
            "https://wiki.archlinux.org/index.php?search=niri+wm"
        );
        assert_eq!(resolve_url("github.com/foo"), "https://github.com/foo");
        assert_eq!(resolve_url("localhost:8080"), "http://localhost:8080");
        assert_eq!(
            resolve_url("rust traits"),
            "https://duckduckgo.com/?q=rust+traits"
        );
        assert_eq!(resolve_url("aw"), "https://duckduckgo.com/?q=aw");
        assert_eq!(resolve_url("about:blank"), "about:blank");
        assert_eq!(resolve_url("c++"), "https://duckduckgo.com/?q=c%2B%2B");
    }

    #[test]
    fn edit_lines_rewrites_entries() {
        let path = std::env::temp_dir().join(format!("rembrowser-test-{}", std::process::id()));
        std::fs::write(&path, "a\n\nb\nc\n").unwrap();
        // Entries are the non-blank lines: drop "b" (entry 1), change "c".
        edit_lines(&path, |i, l| match i {
            1 => None,
            2 => Some(format!("{l}!")),
            _ => Some(l.to_string()),
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nc!\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn set_forms() {
        exec("set scroll-step=60");
        assert_eq!(opt_num("scroll-step", 0), 60);
        exec("set hint-follow-last");
        assert!(opt_bool("hint-follow-last", false));
        exec("set incsearch!");
        assert!(!opt_bool("incsearch", true));
    }
}
