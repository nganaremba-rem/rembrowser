//! rembrowser — vim-native Chromium (CEF) browser.
//!
//! The same binary is re-executed by CEF for renderer/GPU/utility processes;
//! `execute_process` returns >= 0 in those and we exit immediately.

mod adblock;
mod commands;
mod downloads;
mod editor;
mod keys;
mod permissions;
mod popup;
mod privacy;
mod tabs;
mod ui;
mod vim;

use cef::*;

const HOME_URL: &str = "https://duckduckgo.com/?kae=d";

fn data_dir() -> String {
    let base = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| format!("{}/.local/share", std::env::var("HOME").unwrap_or_default()));
    format!("{base}/rembrowser")
}

fn main() {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let args = args::Args::new();
    let code = execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    if code >= 0 {
        std::process::exit(code);
    }

    // Browser process only from here on.
    let root = data_dir();
    let profile = format!("{root}/Default");
    privacy::write_widevine_hint(&profile);
    adblock::start();
    // Before CEF reads its switches: the config can turn on dark-mode.
    commands::load_config();

    let url = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .map_or_else(|| commands::resolve_url(""), |a| commands::resolve_url(&a));

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

    let mut app = RemApp::new(url);
    assert_eq!(
        initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut(),
        ),
        1,
        "CEF initialize failed"
    );
    run_message_loop();
    shutdown();
}

wrap_app! {
    struct RemApp {
        url: String,
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
            Some(RemBrowserProcessHandler::new(self.url.clone()))
        }
    }
}

wrap_browser_process_handler! {
    struct RemBrowserProcessHandler {
        url: String,
    }

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            privacy::apply_prefs();
            tabs::create_window(self.url.clone());
        }
    }
}
