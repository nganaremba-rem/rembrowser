//! Window + tabs. Every tab is a CEF BrowserView child of one top-level Window
//! (fill layout); only the active one is visible.
//!
//! All CEF Views calls happen on the UI thread, so tab state lives in a thread_local
//! (no locks). The adblock hook runs on the IO thread and touches no tab state.

use crate::adblock;
use cef::*;
use std::cell::{Cell, RefCell};

pub const APP_ID: &str = "rembrowser";
/// Catppuccin Mocha base: no white flash while a tab loads.
pub const BG: u32 = 0xFF1E_1E2E;

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
    static TABS: RefCell<Vec<BrowserView>> = const { RefCell::new(Vec::new()) };
    static ACTIVE: Cell<usize> = const { Cell::new(0) };
    /// Disposition seen in on_before_popup, consumed by on_popup_browser_view_created.
    static NEXT_POPUP_BACKGROUND: Cell<bool> = const { Cell::new(false) };
}

/// cef-rs bug (152.3): when an out-struct is written back to C, an *owned* CefString
/// (e.g. `"x".into()`) becomes zeroed, so CEF sees "". A *borrowed* one survives,
/// so point it at a static UTF-16 buffer (CEF copies it).
fn app_id_str() -> CefString {
    static BUF: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    let buf = BUF.get_or_init(|| APP_ID.encode_utf16().collect());
    sys::_cef_string_utf16_t { str_: buf.as_ptr().cast_mut(), length: buf.len(), dtor: None }.into()
}

pub fn create_window(url: String) {
    let mut delegate = RemWindowDelegate::new(url);
    window_create_top_level(Some(&mut delegate));
}

fn browser_settings() -> BrowserSettings {
    BrowserSettings { background_color: BG, ..Default::default() }
}

pub fn open(url: &str, background: bool) {
    let mut client = RemClient::new();
    let mut delegate = RemBrowserViewDelegate::new();
    let bv = browser_view_create(
        Some(&mut client),
        Some(&url.into()),
        Some(&browser_settings()),
        None,
        None,
        Some(&mut delegate),
    );
    if let Some(bv) = bv {
        add(bv, background);
    }
}

fn add(bv: BrowserView, background: bool) {
    let Some(win) = WINDOW.with_borrow(|w| w.clone()) else { return };
    let mut view = View::from(&bv);
    view.set_visible(0);
    win.add_child_view(Some(&mut view));
    let i = TABS.with_borrow_mut(|t| {
        t.push(bv);
        t.len() - 1
    });
    if !background || i == 0 {
        select(i);
    }
}

pub fn select(i: usize) {
    TABS.with_borrow(|tabs| {
        let Some(bv) = tabs.get(i) else { return };
        if let Some(old) = tabs.get(ACTIVE.get()) {
            View::from(old).set_visible(0);
        }
        View::from(bv).set_visible(1);
        bv.request_focus();
        ACTIVE.set(i);
        set_window_title(bv);
    });
}

/// Relative move, wrapping (gt / gT).
pub fn cycle(delta: isize) {
    let n = TABS.with_borrow(|t| t.len()) as isize;
    if n > 0 {
        select((ACTIVE.get() as isize + delta).rem_euclid(n) as usize);
    }
}

pub fn close_active() {
    let host = TABS.with_borrow(|t| t.get(ACTIVE.get()).and_then(|bv| bv.browser()?.host()));
    if let Some(h) = host {
        h.close_browser(0);
    }
}

fn index_of(browser: &mut Browser) -> Option<usize> {
    TABS.with_borrow(|t| {
        t.iter().position(|bv| bv.browser().is_some_and(|b| b.is_same(Some(browser)) != 0))
    })
}

fn is_active(browser: &mut Browser) -> bool {
    index_of(browser) == Some(ACTIVE.get())
}

fn set_window_title(bv: &BrowserView) {
    let title = bv
        .browser()
        .and_then(|b| b.host()?.visible_navigation_entry())
        .map(|e| CefString::from(&e.title()).to_string())
        .unwrap_or_default();
    if let Some(w) = WINDOW.with_borrow(|w| w.clone()) {
        w.set_title(Some(&title.as_str().into()));
    }
}

fn on_tab_closed(browser: &mut Browser) {
    let Some(i) = index_of(browser) else { return };
    let removed = TABS.with_borrow_mut(|t| t.remove(i));
    if let Some(w) = WINDOW.with_borrow(|w| w.clone()) {
        w.remove_child_view(Some(&mut View::from(&removed)));
    }
    let n = TABS.with_borrow(|t| t.len());
    if n == 0 {
        // Last tab gone: close the window, which quits (vimb `:q` on the last tab).
        if let Some(w) = WINDOW.with_borrow(|w| w.clone()) {
            w.close();
        }
        return;
    }
    let active = ACTIVE.get();
    if i < active {
        ACTIVE.set(active - 1);
    } else if i == active {
        // Closed the visible tab: show its left neighbour, like vim's bufferline.
        ACTIVE.set(usize::MAX);
        select(i.saturating_sub(1).min(n - 1));
    }
}

wrap_window_delegate! {
    struct RemWindowDelegate {
        url: String,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            Size { width: 1280, height: 800 }
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn window_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::ALLOY
        }

        fn on_window_created(&self, window: Option<&mut Window>) {
            let Some(window) = window else { return };
            window.set_to_fill_layout();
            WINDOW.with_borrow_mut(|w| *w = Some(window.clone()));
            open(&self.url, false);
            window.show();
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            WINDOW.with_borrow_mut(|w| w.take());
            quit_message_loop();
        }

        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            // Ask every tab (runs beforeunload). Each closed tab hits on_tab_closed;
            // the last one closes the window again, which then passes.
            let hosts: Vec<_> = TABS.with_borrow(|t| t.iter().filter_map(|bv| bv.browser()?.host()).collect());
            hosts.iter().fold(1, |all, h| all & h.try_close_browser())
        }

        fn linux_window_properties(
            &self,
            _window: Option<&mut Window>,
            properties: Option<&mut LinuxWindowProperties>,
        ) -> i32 {
            let Some(p) = properties else { return 0 };
            p.wayland_app_id = app_id_str();
            p.wm_class_class = app_id_str();
            p.wm_class_name = app_id_str();
            1
        }
    }
}

wrap_browser_view_delegate! {
    struct RemBrowserViewDelegate {}

    impl ViewDelegate {}

    impl BrowserViewDelegate {
        // Alloy = CEF without Chrome's UI layer. Chrome style allows only one
        // BrowserView per window (no tabs) and preloads hidden WebUI renderers.
        fn browser_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::ALLOY
        }

        fn on_popup_browser_view_created(
            &self,
            _browser_view: Option<&mut BrowserView>,
            popup_browser_view: Option<&mut BrowserView>,
            _is_devtools: i32,
        ) -> i32 {
            // Popups (target=_blank, window.open, ctrl/middle-click) become tabs.
            // Keeps window.opener, so OAuth "Sign in with X" popups still work.
            let Some(bv) = popup_browser_view else { return 0 };
            add(bv.clone(), NEXT_POPUP_BACKGROUND.replace(false));
            1
        }
    }
}

wrap_client! {
    struct RemClient;

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(RemDisplayHandler::new())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(RemLifeSpanHandler::new())
        }

        fn keyboard_handler(&self) -> Option<KeyboardHandler> {
            Some(RemKeyboardHandler::new())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(RemRequestHandler::new())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(RemDownloadHandler::new())
        }
    }
}

wrap_download_handler! {
    struct RemDownloadHandler;

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
            _download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> i32 {
            let Some(cb) = callback else { return 0 };
            let name = suggested_name.map(|n| n.to_string()).unwrap_or_default();
            let path = download_path(&name);
            cb.cont(Some(&path.to_string_lossy().as_ref().into()), 0);
            1
        }
    }
}

/// ~/Downloads/<name>, never overwriting: "file (1).pdf", "file (2).pdf", ...
fn download_path(name: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Downloads");
    let _ = std::fs::create_dir_all(&dir);
    // Strip path separators a hostile server could put in the suggested name.
    let name = match name.rsplit(['/', '\\']).next().filter(|n| !n.is_empty() && *n != "..") {
        Some(n) => n,
        None => "download",
    };
    let p = std::path::Path::new(name);
    let (stem, ext) = (
        p.file_stem().map_or(name.into(), |s| s.to_string_lossy()),
        p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default(),
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
        assert!(p.ends_with("Downloads/passwd") || p.file_name().unwrap().to_string_lossy().starts_with("passwd ("));
        assert!(download_path("..").ends_with("Downloads/download") || download_path("..").to_string_lossy().contains("download ("));
    }
}

wrap_display_handler! {
    struct RemDisplayHandler;

    impl DisplayHandler {
        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            if browser.is_some_and(|b| is_active(b)) {
                if let Some(w) = WINDOW.with_borrow(|w| w.clone()) {
                    w.set_title(title);
                }
            }
        }
    }
}

wrap_life_span_handler! {
    struct RemLifeSpanHandler;

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: i32,
            _target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            _user_gesture: i32,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut i32>,
        ) -> i32 {
            NEXT_POPUP_BACKGROUND.set(target_disposition == WindowOpenDisposition::NEW_BACKGROUND_TAB);
            0
        }

        fn do_close(&self, browser: Option<&mut Browser>) -> i32 {
            // A Views browser is destroyed when its BrowserView leaves the window, so
            // detach it here and return 1. The default (0) would close the whole
            // window, i.e. every tab. on_tab_closed closes the window after the last tab.
            if let Some(b) = browser {
                on_tab_closed(b);
            }
            1
        }
    }
}

wrap_keyboard_handler! {
    struct RemKeyboardHandler;

    impl KeyboardHandler {
        fn on_pre_key_event(
            &self,
            _browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: Option<&mut sys::XEvent>,
            _is_keyboard_shortcut: Option<&mut i32>,
        ) -> i32 {
            let Some(e) = event else { return 0 };
            (e.type_ == KeyEventType::RAWKEYDOWN && temp_shortcut(e.windows_key_code, e.modifiers)) as i32
        }
    }
}

/// Phase-1 stand-in until the vim engine lands: Ctrl-T / Ctrl-W / Ctrl-(Shift-)Tab.
fn temp_shortcut(key: i32, mods: u32) -> bool {
    const SHIFT: u32 = 1 << 1;
    const CTRL: u32 = 1 << 2;
    if mods & CTRL == 0 {
        return false;
    }
    match key {
        0x54 => open(crate::HOME_URL, false),                       // T
        0x57 => close_active(),                                       // W
        0x09 => cycle(if mods & SHIFT != 0 { -1 } else { 1 }),      // Tab
        _ => return false,
    }
    true
}

wrap_request_handler! {
    struct RemRequestHandler;

    impl RequestHandler {
        // Middle-click / Ctrl-click land here in Alloy (no tab strip to handle them).
        fn on_open_urlfrom_tab(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            target_url: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            _user_gesture: i32,
        ) -> i32 {
            let Some(url) = target_url.map(|u| u.to_string()) else { return 0 };
            let background = target_disposition == WindowOpenDisposition::NEW_BACKGROUND_TAB;
            if !background && target_disposition != WindowOpenDisposition::NEW_FOREGROUND_TAB {
                return 0;
            }
            open(&url, background);
            1
        }

        fn resource_request_handler(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _is_navigation: i32,
            _is_download: i32,
            _request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut i32>,
        ) -> Option<ResourceRequestHandler> {
            Some(RemResourceRequestHandler::new())
        }
    }
}

wrap_resource_request_handler! {
    struct RemResourceRequestHandler;

    impl ResourceRequestHandler {
        // IO thread, once per request: keep it cheap.
        fn on_before_resource_load(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            let Some(req) = request else { return ReturnValue::CONTINUE };
            let Some(rtype) = adblock_type(req.resource_type()) else { return ReturnValue::CONTINUE };
            let url = CefString::from(&req.url()).to_string();
            let source = frame
                .map(|f| CefString::from(&f.url()).to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| CefString::from(&req.referrer_url()).to_string());
            if adblock::should_block(&url, &source, rtype) {
                ReturnValue::CANCEL
            } else {
                ReturnValue::CONTINUE
            }
        }
    }
}

/// CEF resource type → adblock request type. None = never block (top-level navigations).
fn adblock_type(t: ResourceType) -> Option<&'static str> {
    Some(match t {
        ResourceType::MAIN_FRAME => return None,
        ResourceType::SUB_FRAME => "sub_frame",
        ResourceType::STYLESHEET => "stylesheet",
        ResourceType::SCRIPT => "script",
        ResourceType::IMAGE | ResourceType::FAVICON => "image",
        ResourceType::FONT_RESOURCE => "font",
        ResourceType::OBJECT | ResourceType::PLUGIN_RESOURCE => "object",
        ResourceType::MEDIA => "media",
        ResourceType::XHR => "xmlhttprequest",
        ResourceType::PING | ResourceType::CSP_REPORT => "ping",
        _ => "other",
    })
}
