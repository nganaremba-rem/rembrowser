//! One rembrowser per user. A second `rembrowser [URL]` / `rembrowser --app=URL`
//! (a link clicked in another app, a web-app launcher) hands its request to the
//! running instance over a unix socket and exits — Chromium can't open the same
//! profile twice.

use crate::{apps, tabs, ui};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub enum Req {
    Open(String),
    /// `--new-window [URL]`, or a second launch without a URL.
    Window(String),
    App(String),
    /// `rembrowser ':tabopen x'`: an ex command for the running browser (scripts, WM keybinds).
    Cmd(String),
}

impl Req {
    fn line(&self) -> String {
        match self {
            Req::Open(u) => format!("open\t{u}\n"),
            Req::Window(u) => format!("window\t{u}\n"),
            Req::App(u) => format!("app\t{u}\n"),
            Req::Cmd(c) => format!("cmd\t{c}\n"),
        }
    }

    fn parse(line: &str) -> Option<Req> {
        // Only the newline: trim_end() would also eat the tab of an empty URL
        // ("open\t\n", a bare `rembrowser`), and the request would be dropped.
        match line.trim_end_matches(['\n', '\r']).split_once('\t')? {
            ("open", u) => Some(Req::Open(u.into())),
            ("window", u) => Some(Req::Window(u.into())),
            ("app", u) => Some(Req::App(u.into())),
            ("cmd", c) => Some(Req::Cmd(c.into())),
            _ => None,
        }
    }
}

fn sock() -> PathBuf {
    // XDG_RUNTIME_DIR is per-user and 0700.
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/tmp/rembrowser-{}", unsafe { libc::getuid() }));
    PathBuf::from(dir).join("rembrowser.sock")
}

/// Give `req` to a running rembrowser. true = it took it (this process exits).
pub fn forward(req: &Req) -> bool {
    UnixStream::connect(sock())
        .and_then(|mut s| s.write_all(req.line().as_bytes()))
        .is_ok()
}

/// Become the instance others forward to.
pub fn listen() {
    let path = sock();
    // Nobody answered forward(): whatever socket file is there is stale.
    let _ = std::fs::remove_file(&path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(listener) = UnixListener::bind(&path) else {
        eprintln!(
            "rembrowser: can't listen on {} (single-instance off)",
            path.display()
        );
        return;
    };
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(conn).read_line(&mut line).is_ok() {
                if let Some(req) = Req::parse(&line) {
                    ui::run_on_ui(0, move || handle(req));
                }
            }
        }
    });
}

pub fn handle(req: Req) {
    match req {
        // Like Chrome: a URL becomes a tab in the window last used, a bare launch
        // a new window.
        Req::Open(url) if !url.is_empty() => tabs::open_or_create(&url),
        Req::Open(url) | Req::Window(url) => {
            tabs::create_window(crate::commands::resolve_url(&url))
        }
        Req::App(url) => apps::open(&url),
        Req::Cmd(cmd) => crate::commands::exec(&cmd),
    }
}

/// argv → request: `--app=URL` / `--app URL` / `--new-window [URL]` / `:command` /
/// `URL` / nothing (home page).
pub fn from_args(args: &[String]) -> Req {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(u) = a.strip_prefix("--app=") {
            return Req::App(u.into());
        }
        if a == "--app" {
            return Req::App(it.next().cloned().unwrap_or_default());
        }
    }
    let arg = args.iter().find(|a| !a.starts_with("--"));
    match arg.and_then(|a| a.strip_prefix(':')) {
        Some(cmd) => Req::Cmd(cmd.into()),
        None if args.iter().any(|a| a == "--new-window") => {
            Req::Window(arg.cloned().unwrap_or_default())
        }
        None => Req::Open(arg.cloned().unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_and_wire_format() {
        let a = |v: &[&str]| from_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            a(&["--app=https://web.whatsapp.com"]),
            Req::App("https://web.whatsapp.com".into())
        );
        assert_eq!(a(&["--app", "discord.com"]), Req::App("discord.com".into()));
        assert_eq!(a(&["https://x.org"]), Req::Open("https://x.org".into()));
        assert_eq!(a(&[]), Req::Open(String::new()));
        assert_eq!(a(&[":tabopen x.org"]), Req::Cmd("tabopen x.org".into()));
        assert_eq!(a(&["--new-window"]), Req::Window(String::new()));
        assert_eq!(a(&["--new-window", "x.org"]), Req::Window("x.org".into()));
        for r in [Req::App("https://a.b/c?d".into()), Req::Open(String::new()), Req::Window(String::new())] {
            assert_eq!(Req::parse(&r.line()), Some(r));
        }
    }
}
