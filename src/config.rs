//! ~/.config/rembrowser/config.yaml — the user config. Applied on top of the built-in
//! defaults; saved edits apply live (no restart). `rembrowser config` / `:config` open it.
//!
//!   source:   [~/.config/vimb/config]      # vimb-syntax files, loaded first
//!   settings: { home-page: home, status-bar: false, ... }   # = :set
//!   search:   { default: dd, engines: { aw: "https://...$0" } }
//!   keys:     { normal: { "<Space>sb": ":tabs<CR>" }, insert: {}, command: {} }
//!   (a key can also be { run: ":tabs<CR>", desc: "tabs picker" } for the :keys sheet)

use crate::{commands, ui};
use std::path::PathBuf;
use std::time::SystemTime;
use yaml_rust2::{Yaml, YamlLoader};

pub fn path() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map_or_else(
            |_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"),
            PathBuf::from,
        )
        .join("rembrowser/config.yaml")
}

const TEMPLATE: &str = r#"# rembrowser config — saved changes apply immediately.
# Built-in defaults load first; everything here overrides them.

# vimb-style config files to load first (optional), e.g. a shared vimb config:
source: []
#  - ~/.config/vimb/config

settings:
  home-page: home          # "home" = the homescreen, or any URL
  status-bar: false        # true = always show the status line (false = pops up)
  tab-bar: false           # true = always show the tab bar (false = pops up)
  bookmark-bar: false      # true = show the bookmark bar
  dark-mode: true          # sites get prefers-color-scheme: dark (restart)
  hint-keys: asdfghjkl
  scroll-step: 60          # pixels per j / k
  smooth-scrolling: true   # false = jump instead of glide
  incsearch: true
  confirm-quit: true       # ask before the last tab closes (d, x, :q) and on Ctrl-q
  download-path: ~/Downloads
  geolocation: never
  # editor-command: foot nvim %s     # Ctrl-T in text fields; %s = the file
  # mpv-command: mpv                 # ;m and :mpv

  # Light or full: each of these loads more into the browser. They are read at
  # startup, so restart after changing one.
  adblock: true            # built-in ad & tracker blocking (filter lists in memory)
  drm: true                # Widevine: Netflix, Spotify, ...
  extensions: false        # Chrome extensions; manage them with :extensions

search:
  default: dd              # what plain words in :open search with
  engines:                 # ":open aw niri" → Arch Wiki search for "niri"
    dd: https://duckduckgo.com/?q=$0
    g: https://www.google.com/search?q=$0
    gh: https://github.com/search?q=$0
    aw: https://wiki.archlinux.org/index.php?search=$0
    yt: https://www.youtube.com/results?search_query=$0

keys:
  normal:
    H: gT
    L: gt
    "<Space>sb": { run: ":tabs<CR>", desc: tabs picker }
  insert: {}
  command: {}
"#;

/// Create the file from the template if needed; returns its path.
pub fn ensure() -> PathBuf {
    let p = path();
    if !p.exists() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, TEMPLATE);
    }
    p
}

fn scalar(v: &Yaml) -> Option<String> {
    Some(match v {
        Yaml::String(s) => s.clone(),
        Yaml::Boolean(b) => if *b { "on" } else { "off" }.into(),
        Yaml::Integer(i) => i.to_string(),
        Yaml::Real(r) => r.clone(),
        Yaml::Null => String::new(),
        _ => return None,
    })
}

/// Apply config.yaml. Err = a message for the user (parse error with line).
pub fn load() -> Result<(), String> {
    let p = path();
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Ok(());
    };
    let docs = YamlLoader::load_from_str(&text)
        .map_err(|e| format!("config.yaml line {}: {}", e.marker().line(), e.info()))?;
    let Some(doc) = docs.first() else {
        return Ok(());
    };
    if let Some(files) = doc["source"].as_vec() {
        for f in files.iter().filter_map(Yaml::as_str) {
            commands::exec(&format!("source {f}"));
        }
    }
    if let Some(settings) = doc["settings"].as_hash() {
        for (k, v) in settings {
            if let (Some(k), Some(v)) = (k.as_str(), scalar(v)) {
                commands::exec_one_line(&format!("set {k}={v}"));
            }
        }
    }
    if let Some(engines) = doc["search"]["engines"].as_hash() {
        for (k, v) in engines {
            if let (Some(k), Some(v)) = (k.as_str(), v.as_str()) {
                commands::exec_one_line(&format!("shortcut-add {k}={v}"));
            }
        }
    }
    if let Some(d) = doc["search"]["default"].as_str() {
        commands::exec_one_line(&format!("shortcut-default {d}"));
    }
    for (mode, cmd) in [
        ("normal", "nnoremap"),
        ("insert", "inoremap"),
        ("command", "cnoremap"),
    ] {
        let Some(keys) = doc["keys"][mode].as_hash() else {
            continue;
        };
        for (k, v) in keys {
            let Some(lhs) = scalar(k) else { continue };
            let (rhs, desc) = match v {
                Yaml::Hash(_) => (
                    v["run"].as_str().map(String::from),
                    v["desc"].as_str().map(String::from),
                ),
                _ => (scalar(v), None),
            };
            if let Some(rhs) = rhs.filter(|r| !r.is_empty()) {
                commands::map_with_desc(cmd, &lhs, &rhs, desc);
            }
        }
    }
    Ok(())
}

fn mtime() -> Option<SystemTime> {
    std::fs::metadata(path()).and_then(|m| m.modified()).ok()
}

/// Re-apply config.yaml whenever it's saved (editor, dotfiles sync, ...).
pub fn watch() {
    std::thread::spawn(|| {
        let mut last = mtime();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let now = mtime();
            if now != last && now.is_some() {
                last = now;
                ui::run_on_ui(0, || match load() {
                    Ok(()) => ui::message("config.yaml reloaded", false),
                    Err(e) => ui::message(&e, true),
                });
            }
        }
    });
}

/// `:config` / `rembrowser config`: open in the desktop's default text editor.
pub fn open_in_editor(from_terminal: bool) {
    let p = ensure();
    // In a terminal, $VISUAL/$EDITOR like any CLI tool; otherwise the desktop default.
    if from_terminal {
        if let Some(ed) = std::env::var("VISUAL")
            .ok()
            .or_else(|| std::env::var("EDITOR").ok())
            .filter(|e| !e.is_empty())
        {
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("{ed} \"$1\""))
                .arg("sh")
                .arg(&p)
                .status();
            return;
        }
    }
    // The desktop's default text editor, via gio: unlike plain xdg-open it honours
    // Terminal=true (nvim.desktop & co. get a terminal window).
    let spawned = default_editor_desktop().is_some_and(|d| {
        commands::command("gio")
            .arg("launch")
            .arg(d)
            .arg(&p)
            .spawn()
            .is_ok()
    }) || commands::command("xdg-open").arg(&p).spawn().is_ok();
    if !spawned {
        let msg = format!("open {} in your editor (xdg-open not found)", p.display());
        if from_terminal {
            eprintln!("{msg}")
        } else {
            ui::message(&msg, true)
        }
    }
}

/// The .desktop file of the default text/plain application, if any.
fn default_editor_desktop() -> Option<PathBuf> {
    let out = commands::command("xdg-mime")
        .args(["query", "default", "text/plain"])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let data_home =
        std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    std::iter::once(data_home.as_str())
        .chain(dirs.split(':'))
        .map(|d| PathBuf::from(d).join("applications").join(&name))
        .find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_and_scalars_convert() {
        let docs = YamlLoader::load_from_str(TEMPLATE).expect("template is valid YAML");
        let d = &docs[0];
        assert_eq!(scalar(&d["settings"]["status-bar"]).as_deref(), Some("off"));
        assert_eq!(scalar(&d["settings"]["scroll-step"]).as_deref(), Some("60"));
        assert_eq!(d["search"]["default"].as_str(), Some("dd"));
        assert_eq!(
            d["keys"]["normal"]["<Space>sb"]["desc"].as_str(),
            Some("tabs picker")
        );
    }
}
