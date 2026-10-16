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

fn main() {
    // `rembrowser update` etc.: plain terminal commands, no browser.
    if let Some(code) = cli::run() {
        std::process::exit(code);
    }
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let args = args::Args::new();
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
    if matches!(req, ipc::Req::Cmd(_)) {
        eprintln!("rembrowser: not running (a :command needs a running browser)");
        std::process::exit(1);
    }
    ipc::listen();
    let root = data_dir();
    let profile = format!("{root}/Default");
    // First: the config decides which optional parts start at all (drm, adblock,
    // extensions) and which switches CEF gets (dark-mode).
    commands::load_config();
    if commands::opt_bool("drm", true) {
        widevine::setup(&profile);
    }
    if commands::opt_bool("adblock", true) {
        adblock::start();
    }
    userscripts::load();
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
    run_message_loop();
    shutdown();
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
            privacy::apply_prefs();
            ipc::handle(self.req.clone());
            if is_private() {
                ui::run_on_ui(0, || ui::toast("private window: nothing is saved", false));
            }
        }
    }
}
