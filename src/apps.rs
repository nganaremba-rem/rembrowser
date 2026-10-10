//! Web apps: a site in its own window — no tabs, bars or vim layer — with app_id
//! `rembrowser-app-<site>`, so niri rules, docks and alt-tab treat it as an app.
//!   rembrowser --app=https://web.whatsapp.com     :app [URL]     :app-install [name]
//! Same profile as the browser (you stay logged in). Ordinary links open as tabs in the
//! main browser; window.open popups (sign-in windows) stay with the app.

use crate::{commands, tabs};
use cef::*;
use std::cell::RefCell;

thread_local! {
    /// (window, view, app_id) of every open app window.
    static APPS: RefCell<Vec<(Window, BrowserView, String)>> = const { RefCell::new(Vec::new()) };
}

pub fn app_id(url: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host);
    let id: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("rembrowser-app-{}", id.trim_matches('-'))
}

pub fn open(url: &str) {
    let url = commands::resolve_url(url);
    if let Some(bv) = tabs::new_view(&url) {
        window(bv, app_id(&url));
    }
}

/// A window for an app's view (also for popups an app opens, with the app's id).
pub fn window(bv: BrowserView, app_id: String) {
    let mut d = AppWindowDelegate::new(bv, app_id);
    window_create_top_level(Some(&mut d));
}

fn find<R>(b: &Browser, f: impl FnOnce(&(Window, BrowserView, String)) -> R) -> Option<R> {
    APPS.with_borrow(|a| {
        a.iter()
            .find(|(_, bv, _)| {
                bv.browser()
                    .is_some_and(|x| x.is_same(Some(&mut b.clone())) != 0)
            })
            .map(f)
    })
}

pub fn is_app(b: &Browser) -> bool {
    find(b, |_| ()).is_some()
}

/// app_id of the app window showing this browser (None = a normal tab).
pub fn app_id_of(b: &Browser) -> Option<String> {
    find(b, |(_, _, id)| id.clone())
}

pub fn browser(id: i32) -> Option<Browser> {
    APPS.with_borrow(|a| {
        a.iter()
            .filter_map(|(_, bv, _)| bv.browser())
            .find(|b| b.identifier() == id)
    })
}

pub fn set_title(b: &Browser, title: Option<&CefString>) {
    find(b, |(w, ..)| w.set_title(title));
}

pub fn is_empty() -> bool {
    APPS.with_borrow(|a| a.is_empty())
}

pub fn view_of(b: &Browser) -> Option<BrowserView> {
    find(b, |(_, v, _)| v.clone())
}

fn by_id<R>(id: i32, f: impl FnOnce(&(Window, BrowserView, String)) -> R) -> Option<R> {
    APPS.with_borrow(|a| {
        a.iter()
            .find(|(_, v, _)| v.browser().is_some_and(|b| b.identifier() == id))
            .map(f)
    })
}

/// The window of the app whose page has this browser id.
pub fn window_of(id: i32) -> Option<Window> {
    by_id(id, |(w, ..)| w.clone())
}

/// Close that app's window (again: after its popup finished closing).
pub fn close(id: i32) {
    if let Some(w) = window_of(id) {
        w.close();
    }
}

/// `:app-install [name]`: a launcher entry (menu, fuzzel, dms) for the current page.
pub fn install(name: &str, url: &str, icon_url: Option<String>) -> Result<String, String> {
    let id = app_id(url);
    let home = std::env::var("HOME").unwrap_or_default();
    let data = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let icon = icon_url
        .and_then(|u| {
            let ext = u
                .rsplit('.')
                .next()
                .filter(|e| ["png", "svg", "ico", "webp"].contains(e))
                .unwrap_or("png");
            let path = format!("{data}/rembrowser/icons/{id}.{ext}");
            let _ = std::fs::create_dir_all(format!("{data}/rembrowser/icons"));
            commands::command("curl")
                .args(["-fsSL", "--max-time", "20", "-o", &path, &u])
                .status()
                .ok()?
                .success()
                .then_some(path)
        })
        .unwrap_or_else(|| "rembrowser".into());
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "rembrowser".into());
    let file = format!("{data}/applications/{id}.desktop");
    let _ = std::fs::create_dir_all(format!("{data}/applications"));
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName={name}\nComment={url} (rembrowser web app)\nExec={exe} --app={url}\nIcon={icon}\nTerminal=false\nCategories=Network;\nStartupWMClass={id}\n"
    );
    std::fs::write(&file, entry).map_err(|e| e.to_string())?;
    Ok(file)
}

wrap_window_delegate! {
    struct AppWindowDelegate {
        view: BrowserView,
        app_id: String,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            Size { width: 1100, height: 800 }
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        // cef-rs defaults these to 0 (CEF's C++ defaults are true): a fixed-size
        // window, which niri opens floating.
        fn can_resize(&self, _window: Option<&mut Window>) -> i32 {
            1
        }

        fn can_maximize(&self, _window: Option<&mut Window>) -> i32 {
            1
        }

        fn can_minimize(&self, _window: Option<&mut Window>) -> i32 {
            1
        }
        fn window_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::ALLOY
        }

        fn on_window_created(&self, window: Option<&mut Window>) {
            let Some(w) = window else { return };
            w.set_to_fill_layout();
            w.add_child_view(Some(&mut View::from(&self.view)));
            View::from(&*w).set_background_color(tabs::BG);
            w.show();
            self.view.request_focus();
            APPS.with_borrow_mut(|a| a.push((w.clone(), self.view.clone(), self.app_id.clone())));
        }

        // vim keys act on whichever window has focus.
        fn on_window_activation_changed(&self, _window: Option<&mut Window>, active: i32) {
            if active != 0 {
                tabs::set_app_focus(Some(self.view.clone()));
            }
        }

        fn on_window_destroyed(&self, window: Option<&mut Window>) {
            let Some(w) = window else { return };
            if let Some(b) = self.view.browser() {
                crate::popup::forget(crate::popup::Owner::App(b.identifier()));
            }
            tabs::set_app_focus(None);
            APPS.with_borrow_mut(|a| a.retain(|(x, ..)| View::from(x).is_same(Some(&mut View::from(&*w))) == 0));
            // Last window of any kind gone: quit.
            if is_empty() && tabs::window().is_none() {
                quit_message_loop();
            }
        }

        // The page and (if it was used) this window's command popup.
        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            let Some(b) = self.view.browser() else { return 1 };
            let mut hosts: Vec<_> = b.host().into_iter().collect();
            hosts.extend(crate::popup::host(crate::popup::Owner::App(b.identifier())));
            hosts.iter().fold(1, |all, h| all & h.try_close_browser())
        }

        fn linux_window_properties(
            &self,
            _window: Option<&mut Window>,
            properties: Option<&mut LinuxWindowProperties>,
        ) -> i32 {
            let Some(p) = properties else { return 0 };
            p.wayland_app_id = tabs::static_str(&self.app_id);
            p.wm_class_class = tabs::static_str(&self.app_id);
            p.wm_class_name = tabs::static_str(&self.app_id);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::app_id;

    #[test]
    fn app_ids() {
        assert_eq!(
            app_id("https://web.whatsapp.com/"),
            "rembrowser-app-web-whatsapp-com"
        );
        assert_eq!(
            app_id("https://www.youtube.com/watch?v=1"),
            "rembrowser-app-youtube-com"
        );
        assert_eq!(app_id("app.daily.dev"), "rembrowser-app-app-daily-dev");
    }
}
