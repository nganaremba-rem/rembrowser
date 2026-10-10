//! Window + tabs. Every tab is a CEF BrowserView in the content panel (fill layout);
//! only the active one is visible. The status bar (ui.rs) sits underneath.
//!
//! All CEF Views calls happen on the UI thread, so tab state lives in thread_locals
//! (no locks). Rule: never call into vim/ui while holding a borrow here.

use crate::{adblock, commands, downloads, permissions, popup, ui, vim};
use cef::*;
use std::cell::{Cell, RefCell};

pub const APP_ID: &str = "rembrowser";
/// Catppuccin Mocha base: no white flash while a tab loads.
pub const BG: u32 = 0xFF1E_1E2E;
const PAGE_JS: &str = include_str!("page.js");

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
    static CONTENT: RefCell<Option<Panel>> = const { RefCell::new(None) };
    static TABS: RefCell<Vec<BrowserView>> = const { RefCell::new(Vec::new()) };
    static ACTIVE: Cell<usize> = const { Cell::new(0) };
    /// Disposition seen in on_before_popup, consumed by on_popup_browser_view_created.
    static NEXT_POPUP_BACKGROUND: Cell<bool> = const { Cell::new(false) };
    /// URLs of closed tabs, for `u` / `U`.
    static CLOSED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Status bar right side: find count + load progress.
    static FIND: RefCell<String> = const { RefCell::new(String::new()) };
    static PROGRESS: Cell<u8> = const { Cell::new(100) };
}

/// cef-rs bug (152.3): when an out-struct is written back to C, an *owned* CefString
/// (e.g. `"x".into()`) becomes zeroed, so CEF sees "". A *borrowed* one survives,
/// so point it at a static UTF-16 buffer (CEF copies it).
fn app_id_str() -> CefString {
    static BUF: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    let buf = BUF.get_or_init(|| APP_ID.encode_utf16().collect());
    sys::_cef_string_utf16_t {
        str_: buf.as_ptr().cast_mut(),
        length: buf.len(),
        dtor: None,
    }
    .into()
}

pub fn create_window(url: String) {
    let mut delegate = RemWindowDelegate::new(url);
    window_create_top_level(Some(&mut delegate));
}

pub fn window() -> Option<Window> {
    WINDOW.with_borrow(|w| w.clone())
}

fn active() -> Option<BrowserView> {
    TABS.with_borrow(|t| t.get(ACTIVE.get()).cloned())
}

pub fn with_active(f: impl FnOnce(&Browser)) {
    if let Some(b) = active().and_then(|bv| bv.browser()) {
        f(&b);
    }
}

fn browser_settings() -> BrowserSettings {
    BrowserSettings {
        background_color: BG,
        ..Default::default()
    }
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

pub fn load(url: &str) {
    with_active(|b| {
        if let Some(f) = b.main_frame() {
            f.load_url(Some(&url.into()));
        }
    });
}

fn add(bv: BrowserView, background: bool) {
    let Some(content) = CONTENT.with_borrow(|c| c.clone()) else {
        return;
    };
    let mut view = View::from(&bv);
    view.set_visible(0);
    content.add_child_view(Some(&mut view));
    let i = TABS.with_borrow_mut(|t| {
        t.push(bv);
        t.len() - 1
    });
    if !background || i == 0 {
        select(i);
    } else {
        refresh_right();
    }
}

/// `usize::MAX` = last tab.
pub fn select(i: usize) {
    let (bv, old) = TABS.with_borrow(|t| {
        let i = i.min(t.len().saturating_sub(1));
        (
            t.get(i).cloned().map(|bv| (i, bv)),
            t.get(ACTIVE.get()).cloned(),
        )
    });
    let Some((i, bv)) = bv else { return };
    if let Some(old) = old {
        View::from(&old).set_visible(0);
    }
    View::from(&bv).set_visible(1);
    bv.request_focus();
    ACTIVE.set(i);
    FIND.with_borrow_mut(String::clear);
    set_window_title(&bv);
    ui::set_url(&url_of(&bv));
    vim::reset();
    refresh_right();
}

/// Relative move, wrapping (gt / gT).
pub fn cycle(delta: isize) {
    let n = TABS.with_borrow(|t| t.len()) as isize;
    if n > 0 {
        select((ACTIVE.get() as isize + delta).rem_euclid(n) as usize);
    }
}

pub fn close_active() {
    with_active(|b| {
        if let Some(h) = b.host() {
            h.close_browser(0);
        }
    });
}

pub fn quit_all() {
    if let Some(w) = window() {
        w.close();
    }
}

pub fn reopen(new_tab: bool) {
    match CLOSED.with_borrow_mut(|c| c.pop()) {
        Some(url) if new_tab || TABS.with_borrow(|t| t.is_empty()) => open(&url, false),
        Some(url) => load(&url),
        None => ui::message("no closed tabs", true),
    }
}

pub fn focus_active() {
    if let Some(bv) = active() {
        bv.request_focus();
    }
}

pub fn set_fullscreen(on: bool) {
    if let Some(w) = window() {
        w.set_fullscreen(on as i32);
    }
}

fn url_of(bv: &BrowserView) -> String {
    bv.browser()
        .and_then(|b| b.main_frame())
        .map(|f| CefString::from(&f.url()).to_string())
        .unwrap_or_default()
}

fn title_of(bv: &BrowserView) -> String {
    bv.browser()
        .and_then(|b| b.host()?.visible_navigation_entry())
        .map(|e| CefString::from(&e.title()).to_string())
        .unwrap_or_default()
}

pub fn active_url() -> String {
    active().map(|bv| url_of(&bv)).unwrap_or_default()
}

pub fn active_title() -> String {
    active().map(|bv| title_of(&bv)).unwrap_or_default()
}

/// (title, url) of every tab, in order.
pub fn titles() -> Vec<(String, String)> {
    TABS.with_borrow(|t| t.clone())
        .iter()
        .map(|bv| (title_of(bv), url_of(bv)))
        .collect()
}

/// Run JS in the active tab's main frame, page.js helpers included.
pub fn js(code: &str) {
    with_active(|b| {
        if let Some(f) = b.main_frame() {
            f.execute_java_script(
                Some(&format!("{PAGE_JS};{code}").as_str().into()),
                Some(&"rembrowser://page.js".into()),
                0,
            );
        }
    });
}

pub fn zoom() -> f64 {
    active()
        .and_then(|bv| bv.browser()?.host())
        .map_or(0.0, |h| h.zoom_level())
}

pub fn set_zoom(level: f64) {
    with_active(|b| {
        if let Some(h) = b.host() {
            h.set_zoom_level(level);
        }
    });
}

/// Real (trusted) left click at CSS-pixel coordinates from page.js.
pub fn click(x: f64, y: f64) {
    // CEF mouse coordinates are view (DIP) pixels = CSS pixels × page zoom.
    // ponytail: ignores pinch-zoom; scale by visualViewport if that ever matters.
    let factor = 1.2f64.powf(zoom());
    let ev = MouseEvent {
        x: (x * factor) as i32,
        y: (y * factor) as i32,
        modifiers: 0,
    };
    with_active(|b| {
        if let Some(h) = b.host() {
            h.send_mouse_move_event(Some(&ev), 0);
            h.send_mouse_click_event(Some(&ev), MouseButtonType::LEFT, 0, 1);
            h.send_mouse_click_event(Some(&ev), MouseButtonType::LEFT, 1, 1);
        }
    });
}

pub fn find(text: &str, forward: bool, next: bool) {
    with_active(|b| {
        if let Some(h) = b.host() {
            h.find(Some(&text.into()), forward as i32, 0, next as i32);
        }
    });
}

pub fn stop_find() {
    FIND.with_borrow_mut(String::clear);
    refresh_right();
    with_active(|b| {
        if let Some(h) = b.host() {
            h.stop_finding(1);
        }
    });
}

pub fn refresh_tabbar() {
    let titles: Vec<String> = TABS
        .with_borrow(|t| t.clone())
        .iter()
        .map(title_of)
        .collect();
    ui::set_tabs(&titles, ACTIVE.get());
}

fn refresh_right() {
    refresh_tabbar();
    let n = TABS.with_borrow(|t| t.len());
    let find = FIND.with_borrow(|f| f.clone());
    let progress = PROGRESS.get();
    let load = if progress < 100 {
        format!(" {progress}%")
    } else {
        String::new()
    };
    ui::set_right(format!("{find} [{}/{n}]{load}", ACTIVE.get() + 1).trim_start());
}

/// The status bar is built after the first page loads: creating its widgets costs
/// ~100 ms of UI thread, which at startup lands exactly when the first page commits
/// (measured: launch→loaded 0.55 s with it up front, 0.45 s without).
fn build_bar() {
    let Some(w) = window() else { return };
    if ui::is_built() {
        return;
    }
    let (tabbar, status) = ui::build();
    w.add_child_view_at(Some(&mut View::from(&tabbar)), 0);
    w.add_child_view(Some(&mut View::from(&status)));
    ui::set_bar_visible(commands::opt_bool("status-bar", true));
    ui::set_tabbar_visible(commands::opt_bool("tab-bar", true));
    ui::recolor();
    popup::init(&w);
    select(ACTIVE.get());
}

fn index_of(browser: &mut Browser) -> Option<usize> {
    TABS.with_borrow(|t| {
        t.iter()
            .position(|bv| bv.browser().is_some_and(|b| b.is_same(Some(browser)) != 0))
    })
}

fn is_active(browser: &mut Browser) -> bool {
    index_of(browser) == Some(ACTIVE.get())
}

fn set_window_title(bv: &BrowserView) {
    let title = title_of(bv);
    if let Some(w) = window() {
        w.set_title(Some(&title.as_str().into()));
    }
}

fn on_tab_closed(browser: &mut Browser) {
    let Some(i) = index_of(browser) else { return };
    let removed = TABS.with_borrow_mut(|t| t.remove(i));
    let url = url_of(&removed);
    if !url.is_empty() && url != "about:blank" {
        CLOSED.with_borrow_mut(|c| c.push(url));
    }
    if let Some(c) = CONTENT.with_borrow(|c| c.clone()) {
        c.remove_child_view(Some(&mut View::from(&removed)));
    }
    let n = TABS.with_borrow(|t| t.len());
    if n == 0 {
        // Last tab gone: close the window, which quits (vimb `:q` on the last tab).
        quit_all();
        return;
    }
    let active = ACTIVE.get();
    if i < active {
        ACTIVE.set(active - 1);
        refresh_right();
    } else if i == active {
        // Closed the visible tab: show its left neighbour, like vim's bufferline.
        ACTIVE.set(usize::MAX);
        select(i.saturating_sub(1).min(n - 1));
    } else {
        refresh_right();
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
        // Runs before the first layout and on every OS/GTK theme switch. Views get
        // their theme colours re-applied *after* this (and whenever they are added),
        // overwriting any earlier set_background_color — so per-view colours are
        // re-applied on the next tick.
        fn on_theme_colors_changed(&self, window: Option<&mut Window>, _chrome_theme: i32) {
            if let Some(w) = window {
                ui::theme(w);
            }
        }

        fn window_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::ALLOY
        }

        fn on_window_created(&self, window: Option<&mut Window>) {
            let Some(window) = window else { return };
            // [ tabs (flex) ] over [ status bar ].
            let layout = window.set_to_box_layout(Some(&BoxLayoutSettings {
                cross_axis_alignment: AxisAlignment::STRETCH,
                ..Default::default()
            }));
            let content = panel_create(None).expect("panel");
            content.set_to_fill_layout();
            window.add_child_view(Some(&mut View::from(&content)));
            if let Some(l) = layout {
                l.set_flex_for_view(Some(&mut View::from(&content)), 1);
            }
            View::from(&*window).set_background_color(BG);
            WINDOW.with_borrow_mut(|w| *w = Some(window.clone()));
            CONTENT.with_borrow_mut(|c| *c = Some(content));
            open(&self.url, false);
            window.show();
            // Fallback for slow first pages; normally the first load_end builds it.
            ui::run_on_ui(1000, build_bar);
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            WINDOW.with_borrow_mut(|w| w.take());
            quit_message_loop();
        }

        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            // Ask every tab (runs beforeunload). Each closed tab hits on_tab_closed;
            // the last one closes the window again, which then passes.
            // The popup overlay too: a browser left open at shutdown trips a CHECK.
            let mut hosts: Vec<_> = TABS.with_borrow(|t| t.iter().filter_map(|bv| bv.browser()?.host()).collect());
            hosts.extend(popup::host());
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
            // Popups (target=_blank, window.open) become tabs. Keeps window.opener,
            // so OAuth "Sign in with X" popups still work.
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

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(RemLoadHandler::new())
        }

        fn find_handler(&self) -> Option<FindHandler> {
            Some(RemFindHandler::new())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(downloads::RemDownloadHandler::new())
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(permissions::RemPermissionHandler::new())
        }
    }
}

wrap_display_handler! {
    struct RemDisplayHandler;

    impl DisplayHandler {
        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            if browser.is_some_and(|b| is_active(b)) {
                if let Some(w) = window() {
                    w.set_title(title);
                }
            }
            refresh_tabbar();
        }

        fn on_address_change(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            let main = frame.is_some_and(|f| f.is_main() != 0);
            if main && browser.is_some_and(|b| is_active(b)) {
                ui::set_url(&url.map(|u| u.to_string()).unwrap_or_default());
            }
        }

        fn on_loading_progress_change(&self, browser: Option<&mut Browser>, progress: f64) {
            if browser.is_some_and(|b| is_active(b)) {
                PROGRESS.set((progress * 100.0) as u8);
                refresh_right();
            }
        }

        // page.js replies arrive as "\x01rem\t<type>\t<fields>" console messages.
        fn on_console_message(
            &self,
            browser: Option<&mut Browser>,
            _level: LogSeverity,
            message: Option<&CefString>,
            _source: Option<&CefString>,
            _line: i32,
        ) -> i32 {
            let msg = message.map(|m| m.to_string()).unwrap_or_default();
            let Some(body) = msg.strip_prefix("\u{1}rem\t") else { return 0 };
            // Cosmetic adblock runs in every tab, not just the visible one.
            if let (Some(rest), Some(f)) = (body.strip_prefix("cls\t"), browser.as_ref().and_then(|b| b.main_frame())) {
                let (classes, ids) = rest.split_once('\t').unwrap_or((rest, ""));
                let css = adblock::class_id_css(&CefString::from(&f.url()).to_string(), classes, ids);
                if !css.is_empty() {
                    let js = format!("__rem.cosmetic({},false)", popup::js_str(&css));
                    f.execute_java_script(Some(&js.as_str().into()), Some(&"rembrowser://page.js".into()), 0);
                }
                return 1;
            }
            if browser.is_some_and(|b| is_active(b)) {
                let fields: Vec<&str> = body.split('\t').collect();
                vim::on_page_msg(&fields);
            }
            1
        }
    }
}

wrap_load_handler! {
    struct RemLoadHandler;

    impl LoadHandler {
        // Cosmetic adblock: site CSS before the page paints; page.js starts reporting
        // class/id names for the generic rules.
        fn on_load_start(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType) {
            let Some(f) = frame.filter(|f| f.is_main() != 0) else { return };
            let url = CefString::from(&f.url()).to_string();
            if !url.starts_with("http") {
                return;
            }
            let (css, generic) = adblock::page_css(&url);
            let js = format!("{PAGE_JS};__rem.cosmetic({},{generic})", popup::js_str(&css));
            f.execute_java_script(Some(&js.as_str().into()), Some(&"rembrowser://page.js".into()), 0);
        }

        fn on_load_end(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _http_status_code: i32) {
            let Some(f) = frame.filter(|f| f.is_main() != 0) else { return };
            // Install page.js early so focus → INSERT shows without a key press.
            f.execute_java_script(Some(&PAGE_JS.into()), Some(&"rembrowser://page.js".into()), 0);
            let url = CefString::from(&f.url()).to_string();
            let title = browser
                .and_then(|b| b.host()?.visible_navigation_entry())
                .map(|e| CefString::from(&e.title()).to_string())
                .unwrap_or_default();
            commands::record_history(&url, &title);
            if !ui::is_built() {
                ui::run_on_ui(0, build_bar);
            }
        }
    }
}

wrap_find_handler! {
    struct RemFindHandler;

    impl FindHandler {
        fn on_find_result(
            &self,
            browser: Option<&mut Browser>,
            _identifier: i32,
            count: i32,
            _selection_rect: Option<&Rect>,
            active_match_ordinal: i32,
            final_update: i32,
        ) {
            if final_update != 0 && browser.is_some_and(|b| is_active(b)) {
                let text = if count == 0 {
                    "no match".into()
                } else {
                    // Chromium reports ordinal 0 for a fresh (incsearch) session though match 1 is active.
                    format!("{}/{count}", active_match_ordinal.max(1)) };
                popup::set_count(&text);
                FIND.with_borrow_mut(|f| *f = text);
                refresh_right();
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
        // Before the page sees the key: swallowing the keydown also drops its char event.
        fn on_pre_key_event(
            &self,
            _browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: Option<&mut sys::XEvent>,
            _is_keyboard_shortcut: Option<&mut i32>,
        ) -> i32 {
            let Some(e) = event else { return 0 };
            (e.type_ == KeyEventType::RAWKEYDOWN && vim::on_key(e)) as i32
        }
    }
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
            Some(adblock::hook::RemResourceRequestHandler::new())
        }
    }
}
