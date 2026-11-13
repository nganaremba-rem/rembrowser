//! rembrowser — vim-native Chromium (CEF) browser.
//!
//! The same binary is re-executed by CEF for renderer/GPU/utility processes;
//! `execute_process` returns >= 0 in those and we exit immediately.

mod adblock;
mod apps;
mod cli;
mod commands;
mod config;
mod downloads;
mod editor;
mod extensions;
mod home;
mod ipc;
mod keys;
mod permissions;
mod pickers;
mod popup;
mod privacy;
mod session;
mod tabs;
mod ui;
mod userscripts;
mod vim;
mod widevine;

use cef::*;

/// The profile of normal windows.
fn real_data_dir() -> String {
    let base = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| format!("{}/.local/share", std::env::var("HOME").unwrap_or_default()));
    format!("{base}/rembrowser")
}

/// Where this process keeps its profile and data files: the real one, or in a private
/// window (`--private`) a throwaway folder in RAM that is deleted on exit.
fn data_dir() -> String {
    std::env::var("REMBROWSER_PRIVATE_DIR").unwrap_or_else(|_| real_data_dir())
}

fn is_private() -> bool {
    std::env::var_os("REMBROWSER_PRIVATE_DIR").is_some()
}

/// `--private`: own profile folder (tmpfs, mode 700) and own single-instance socket,
/// so it runs beside the normal browser and leaves nothing behind.
fn enter_private() {
    use std::os::unix::fs::PermissionsExt;
    let run = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let dir = format!("{run}/rembrowser-private-{}", std::process::id());
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    // SAFETY: single-threaded here, before the browser starts any thread.
    unsafe { std::env::set_var("REMBROWSER_PRIVATE_DIR", dir) };
}

/// A panic inside a CEF callback aborts the whole browser with no trace. Write one
/// (message, place, backtrace) to `<data>/panic.log` so the next crash has evidence.
fn install_panic_log() {
    std::panic::set_hook(Box::new(|info| {
        use std::io::Write;
        let bt = std::backtrace::Backtrace::force_capture();
        let line = format!("--- panic ---\n{info}\n{bt}\n");
        eprintln!("{line}");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!("{}/panic.log", real_data_dir()))
        {
            let _ = f.write_all(line.as_bytes());
        }
    }));
}

fn timing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("REMBROWSER_TIMING").is_some())
}

/// `REMBROWSER_TIMING=1`: print how long each startup stage took (ms since main).
pub fn mark(label: &str) {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let t0 = T0.get_or_init(std::time::Instant::now);
    if timing() {
        eprintln!("[timing] {:>5} ms  {label}", t0.elapsed().as_millis());
    }
}

/// `REMBROWSER_TIMING=1`: `let _t = crate::span("name");` at the top of a function prints
/// "slow name N ms" when it took 15 ms or more of the UI thread. Does nothing otherwise.
pub struct Span(&'static str, Option<std::time::Instant>);

pub fn span(label: &'static str) -> Span {
    Span(label, timing().then(std::time::Instant::now))
}

impl Drop for Span {
    fn drop(&mut self) {
        let ms = self.1.map_or(0, |t| t.elapsed().as_millis());
        if ms >= 15 {
            mark(&format!("slow {} {ms} ms", self.0));
        }
    }
}

fn main() {
    mark("main");
    install_panic_log();
    // `rembrowser update` etc.: plain terminal commands, no browser.
    if let Some(code) = cli::run() {
        std::process::exit(code);
    }
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let args = args::Args::new();
    mark("before execute_process");
    let code = execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    if code >= 0 {
        std::process::exit(code);
    }

    // Browser process only from here on. Private window? Own profile first: the
    // single-instance socket below depends on it.
    if std::env::args().any(|a| a == "--private") {
        enter_private();
    }
    // Already running? Hand it the URL / app.
    let req = ipc::from_args(&std::env::args().skip(1).collect::<Vec<_>>());
    match ipc::forward(&req) {
        ipc::Forwarded::Taken => std::process::exit(0),
        ipc::Forwarded::Refused => {
            ipc::refused();
            std::process::exit(1);
        }
        ipc::Forwarded::NoInstance => {}
    }
    // `--background` is the one command that may start the browser (no window).
    if matches!(&req, ipc::Req::Cmd(c) if c != "background") {
        eprintln!("rembrowser: not running (a :command needs a running browser)");
        std::process::exit(1);
    }
    mark("ipc forwarded/none");
    ipc::listen();
    mark("ipc listening");
    if !is_private() {
        session::start();
    }
    let root = data_dir();
    let profile = format!("{root}/Default");
    // First: the config decides which optional parts start at all (drm, adblock,
    // extensions) and which switches CEF gets (dark-mode).
    commands::load_config();
    mark("config loaded");
    if commands::opt_bool("drm", true) {
        widevine::setup(&profile);
    }
    mark("widevine setup");
    if commands::opt_bool("adblock", true) {
        adblock::start();
    }
    mark("adblock started");
    userscripts::load();
    mark("userscripts loaded");
    config::watch();

    let settings = Settings {
        root_cache_path: root.as_str().into(),
        // Persistent profile; an empty cache_path would mean incognito.
        cache_path: profile.as_str().into(),
        background_color: tabs::BG,
        remote_debugging_port: std::env::var("REMBROWSER_CDP")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0),
        ..Default::default()
    };

    let mut app = RemApp::new(req);
    mark("before cef initialize");
    let started = initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if started != 1 {
        // The reason (no display, profile in use, ...) is in Chromium's log lines above.
        eprintln!("rembrowser: the browser engine failed to start (see the errors above)");
        if is_private() {
            let _ = std::fs::remove_dir_all(data_dir());
        }
        std::process::exit(1);
    }
    mark("cef initialized");
    run_message_loop();
    shutdown();
    if !is_private() {
        session::mark_clean();
    }
    if is_private() {
        let _ = std::fs::remove_dir_all(data_dir());
    }
}

wrap_app! {
    struct RemApp {
        req: ipc::Req,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            // Browser process only (empty process type); children inherit what they need.
            if process_type.is_some_and(|t| !t.to_string().is_empty()) {
                return;
            }
            if let Some(cl) = command_line {
                privacy::apply_switches(cl);
            }
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(RemBrowserProcessHandler::new(self.req.clone()))
        }
    }
}

wrap_browser_process_handler! {
    struct RemBrowserProcessHandler {
        req: ipc::Req,
    }

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            mark("context initialized");
            ui::stall_watch();
            privacy::apply_prefs();
            // A bare launch can bring the last session back (see session.rs).
            let bare = matches!(&self.req, ipc::Req::Open(u) if u.is_empty());
            let restore = bare
                && !is_private()
                && match commands::opt("restore-session").as_deref().unwrap_or("crash") {
                    "always" => true,
                    "crash" => session::crashed(),
                    _ => false,
                };
            if !(restore && tabs::restore_session()) {
                ipc::handle(self.req.clone());
            }
            if is_private() {
                ui::run_on_ui(0, || ui::toast("private window: nothing is saved", false));
            }
        }
    }
}
