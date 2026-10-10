//! Windows + tabs. Every tab is a CEF BrowserView in its window's content panel (fill
//! layout); only the active one is visible. The bars (ui.rs) float above.
//!
//! All CEF Views calls happen on the UI thread, so tab state lives in thread_locals
//! (no locks). Rule: never call into vim/ui while holding a borrow here.

use crate::popup::Owner;
use crate::{adblock, apps, commands, downloads, home, permissions, popup, ui, userscripts, vim};
use cef::*;
use std::cell::{Cell, RefCell};

pub const APP_ID: &str = "rembrowser";
const DEVTOOLS_APP_ID: &str = "rembrowser-devtools";
/// Catppuccin Mocha base: no white flash while a tab loads.
pub const BG: u32 = 0xFF1E_1E2E;
const PAGE_JS: &str = include_str!("page.js");

/// A browser window and its tabs.
struct Win {
    id: u32,
    window: Window,
    content: Panel,
    tabs: Vec<BrowserView>,
    active: usize,
    /// Status bar right side: find count + load progress of the active tab.
    find: String,
    progress: u8,
    /// The quit question (confirm-quit) was answered yes.
    /// ponytail: stays set if a page's beforeunload then cancels the close, so the
    /// next close of this window doesn't ask again.
    confirmed: bool,
}

thread_local! {
    static WINS: RefCell<Vec<Win>> = const { RefCell::new(Vec::new()) };
    /// The browser window that last had focus: commands and new tabs go there.
    static CUR: Cell<u32> = const { Cell::new(0) };
    static NEXT_WIN: Cell<u32> = const { Cell::new(1) };
    /// Disposition seen in on_before_popup, consumed by on_popup_browser_view_created.
    static NEXT_POPUP_BACKGROUND: Cell<bool> = const { Cell::new(false) };
    /// URLs of closed tabs, for `u` / `U`.
    static CLOSED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static FAVICONS: RefCell<std::collections::HashMap<i32, String>> = RefCell::new(std::collections::HashMap::new());
    /// The web-app window with keyboard focus (None = a browser window).
    static APP_FOCUS: RefCell<Option<BrowserView>> = const { RefCell::new(None) };
    /// Open DevTools windows: the last browser window waits for them before closing.
    static DEVTOOLS: RefCell<Vec<Window>> = const { RefCell::new(Vec::new()) };
    /// The last browser window was asked to close (quit_all / WM close).
    /// ponytail: never reset; if a beforeunload cancels the quit, closing DevTools
    /// later re-asks the tabs. Reset on cancel if that ever matters.
    static CLOSING: Cell<bool> = const { Cell::new(false) };
}

/// cef-rs bug (152.3): when an out-struct is written back to C, an *owned* CefString
/// (e.g. `"x".into()`) becomes zeroed, so CEF sees "". A *borrowed* one survives,
/// so point it at a static UTF-16 buffer (CEF copies it).
/// ponytail: leaks the UTF-16 copy; called once per window.
pub fn static_str(s: &str) -> CefString {
    let buf: &'static [u16] = Vec::leak(s.encode_utf16().collect());
    sys::_cef_string_utf16_t {
        str_: buf.as_ptr().cast_mut(),
        length: buf.len(),
        dtor: None,
    }
    .into()
}

fn with_win<R>(id: u32, f: impl FnOnce(&mut Win) -> R) -> Option<R> {
    WINS.with_borrow_mut(|w| w.iter_mut().find(|w| w.id == id).map(f))
}

fn win_ids() -> Vec<u32> {
    WINS.with_borrow(|w| w.iter().map(|w| w.id).collect())
}

/// The browser window commands act on: the last focused one (0 = none open).
pub fn cur() -> u32 {
    let cur = CUR.get();
    WINS.with_borrow(|w| match w.iter().any(|w| w.id == cur) {
        true => cur,
        false => w.last().map_or(0, |w| w.id),
    })
}

/// A new browser window showing `url`.
pub fn create_window(url: String) {
    CLOSING.set(false);
    let id = NEXT_WIN.replace(NEXT_WIN.get() + 1);
    let mut delegate = RemWindowDelegate::new(id, url);
    window_create_top_level(Some(&mut delegate));
}

pub fn window_of(id: u32) -> Option<Window> {
    with_win(id, |w| w.window.clone())
}

pub fn window() -> Option<Window> {
    window_of(cur())
}

fn active_of(win: u32) -> Option<BrowserView> {
    with_win(win, |w| w.tabs.get(w.active).cloned()).flatten()
}

/// What vim keys act on: the focused web app's view, else the active tab.
fn active() -> Option<BrowserView> {
    APP_FOCUS
        .with_borrow(|a| a.clone())
        .or_else(|| active_of(cur()))
}

/// A web-app window got focus (Some) or a browser window did (None).
pub fn set_app_focus(view: Option<BrowserView>) {
    let changed = APP_FOCUS.with_borrow(|a| match (a, &view) {
        (Some(x), Some(y)) => View::from(x).is_same(Some(&mut View::from(y))) == 0,
        (None, None) => false,
        _ => true,
    });
    if changed {
        APP_FOCUS.with_borrow_mut(|a| *a = view);
        vim::reset();
        ui::focus_changed();
    }
}

/// Browser window `id` has the keyboard now.
fn set_cur(id: u32) {
    set_app_focus(None);
    if CUR.replace(id) != id {
        vim::reset();
        ui::focus_changed();
    }
}

/// The window keys go to: the focused web app's, else the current browser window.
pub fn current_owner() -> Owner {
    APP_FOCUS
        .with_borrow(|a| a.as_ref().and_then(|v| v.browser()))
        .map_or_else(|| Owner::Win(cur()), |b| Owner::App(b.identifier()))
}

pub fn current_window() -> Option<Window> {
    match current_owner() {
        Owner::App(id) => apps::window_of(id),
        Owner::Win(id) => window_of(id),
    }
}

/// Page messages (focus, hints) count from the active tab or the focused web app.
fn is_current(browser: &mut Browser) -> bool {
    active()
        .and_then(|bv| bv.browser())
        .is_some_and(|b| b.is_same(Some(browser)) != 0)
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

/// A browser view with all of rembrowser's handlers (tabs and web apps).
pub fn new_view(url: &str) -> Option<BrowserView> {
    let mut client = RemClient::new();
    let mut delegate = RemBrowserViewDelegate::new();
    browser_view_create(
        Some(&mut client),
        Some(&url.into()),
        Some(&browser_settings()),
        None,
        None,
        Some(&mut delegate),
    )
}

/// A tab in the current browser window.
pub fn open(url: &str, background: bool) {
    // From a web app with no browser window open: open one.
    let win = cur();
    if win == 0 {
        create_window(url.to_string());
        return;
    }
    if let Some(bv) = new_view(url) {
        add(win, bv, background);
    }
}

/// From outside (another `rembrowser URL`, a web app's link): a tab in the current
/// browser window, opening one first if only web apps are running.
pub fn open_or_create(url: &str) {
    let url = commands::resolve_url(url);
    match window() {
        Some(w) => {
            open(&url, false);
            w.activate();
        }
        None => create_window(url),
    }
}

pub fn load(url: &str) {
    with_active(|b| {
        if let Some(f) = b.main_frame() {
            f.load_url(Some(&url.into()));
        }
    });
}

fn add(win: u32, bv: BrowserView, background: bool) {
    let Some(content) = with_win(win, |w| w.content.clone()) else {
        return;
    };
    let mut view = View::from(&bv);
    view.set_visible(0);
    content.add_child_view(Some(&mut view));
    let Some(i) = with_win(win, |w| {
        w.tabs.push(bv);
        w.tabs.len() - 1
    }) else {
        return;
    };
    if !background || i == 0 {
        select_in(win, i);
    } else {
        refresh_right(win);
    }
    if i > 0 {
        ui::reveal_tabs();
    }
}

/// `usize::MAX` = last tab.
pub fn select(i: usize) {
    select_in(cur(), i);
}

pub fn select_in(win: u32, i: usize) {
    let picked = with_win(win, |w| {
        let i = i.min(w.tabs.len().saturating_sub(1));
        let bv = w.tabs.get(i).cloned()?;
        let old = w.tabs.get(w.active).cloned();
        let switched = i != w.active;
        w.active = i;
        w.find.clear();
        Some((bv, old, switched))
    });
    let Some((bv, old, switched)) = picked.flatten() else {
        return;
    };
    if switched {
        ui::reveal_tabs();
    }
    if let Some(old) = old {
        View::from(&old).set_visible(0);
    }
    View::from(&bv).set_visible(1);
    bv.request_focus();
    if let Some(w) = window_of(win) {
        w.set_title(Some(&title_of(&bv).as_str().into()));
    }
    ui::set_url_for(Owner::Win(win), &url_of(&bv));
    if win == cur() {
        vim::reset();
    }
    refresh_right(win);
}

/// Relative move, wrapping (gt / gT).
pub fn cycle(delta: isize) {
    let (n, active) = with_win(cur(), |w| (w.tabs.len() as isize, w.active as isize)).unwrap_or((0, 0));
    if n > 0 {
        select((active + delta).rem_euclid(n) as usize);
    }
}

pub fn close_active() {
    with_active(|b| {
        if let Some(h) = b.host() {
            h.close_browser(0);
        }
    });
}

/// Close tab `i` of a window (its close button).
pub fn close_tab(win: u32, i: usize) {
    let host = with_win(win, |w| w.tabs.get(i).and_then(|bv| bv.browser()?.host()));
    if let Some(h) = host.flatten() {
        h.close_browser(0);
    }
}

pub fn close_window(id: u32) {
    if let Some(w) = window_of(id) {
        w.close();
    }
}

/// "Close?" for these windows; yes closes them without asking again.
fn ask_quit(ids: Vec<u32>) {
    let tabs: usize = ids
        .iter()
        .filter_map(|id| with_win(*id, |w| w.tabs.len()))
        .sum();
    let tabs = format!("{tabs} tab{}", if tabs == 1 { "" } else { "s" });
    let text = match ids.len() {
        1 => format!("Close this window? ({tabs})"),
        n => format!("Quit rembrowser? ({n} windows, {tabs})"),
    };
    popup::confirm(&text, "Quit", move |yes| {
        if yes {
            for id in ids {
                with_win(id, |w| w.confirmed = true);
                close_window(id);
            }
        }
    });
}

/// Windows that would lose tabs without the user having said yes (confirm-quit).
fn unconfirmed() -> Vec<u32> {
    if !commands::opt_bool("confirm-quit", true) {
        return Vec::new();
    }
    WINS.with_borrow(|w| {
        w.iter()
            .filter(|w| !w.tabs.is_empty() && !w.confirmed)
            .map(|w| w.id)
            .collect()
    })
}

/// Close every browser window (asking once), which quits unless web apps are open.
pub fn quit_all() {
    if unconfirmed().is_empty() {
        win_ids().into_iter().for_each(close_window);
    } else {
        ask_quit(win_ids());
    }
}

pub fn reopen(new_tab: bool) {
    let empty = active_of(cur()).is_none();
    match CLOSED.with_borrow_mut(|c| c.pop()) {
        Some(url) if new_tab || empty => open(&url, false),
        Some(url) => load(&url),
        None => ui::message("no closed tabs", true),
    }
}

/// Keyboard back to a window's page (after its hidden popup loaded and took focus).
pub fn focus_owner(owner: Owner) {
    let view = match owner {
        Owner::App(id) => apps::browser(id).and_then(|b| apps::view_of(&b)),
        Owner::Win(id) => active_of(id),
    };
    if let Some(v) = view {
        v.request_focus();
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

pub fn active_favicon() -> Option<String> {
    let id = active_id()?;
    FAVICONS.with_borrow(|f| f.get(&id).cloned())
}

pub fn active_title() -> String {
    active().map(|bv| title_of(&bv)).unwrap_or_default()
}

/// (title, url) of every tab of the current window, in order.
pub fn titles() -> Vec<(String, String)> {
    with_win(cur(), |w| w.tabs.clone())
        .unwrap_or_default()
        .iter()
        .map(|bv| (title_of(bv), url_of(bv)))
        .collect()
}

/// Browser id of the active tab (stable while the tab lives; Send, unlike Browser).
pub fn active_id() -> Option<i32> {
    active().and_then(|bv| bv.browser()).map(|b| b.identifier())
}

/// Like js(), in the tab or web app with this browser id (if it still exists).
pub fn js_in(id: i32, code: &str) {
    let b = WINS
        .with_borrow(|w| {
            w.iter()
                .flat_map(|w| &w.tabs)
                .filter_map(|bv| bv.browser())
                .find(|b| b.identifier() == id)
        })
        .or_else(|| apps::browser(id));
    if let Some(f) = b.and_then(|b| b.main_frame()) {
        f.execute_java_script(
            Some(&format!("{PAGE_JS};{code}").as_str().into()),
            Some(&"rembrowser://page.js".into()),
            0,
        );
    }
}

/// Run JS in the active tab *as a user gesture* (DevTools protocol, in-process: no
/// debugging port). For APIs that need one, like requestPictureInPicture.
pub fn js_gesture(code: &str) {
    with_active(|b| {
        let (Some(h), Some(params)) = (b.host(), dictionary_value_create()) else {
            return;
        };
        params.set_string(Some(&"expression".into()), Some(&code.into()));
        params.set_bool(Some(&"userGesture".into()), 1);
        h.execute_dev_tools_method(
            0,
            Some(&"Runtime.evaluate".into()),
            Some(&mut params.clone()),
        );
    });
}

/// `gp` / `:pip`: picture-in-picture for the page's main video (toggle).
pub fn pip() {
    js_gesture(
        "(() => { if (document.pictureInPictureElement) return document.exitPictureInPicture();\
         const vs = [...document.querySelectorAll('video')].filter((v) => v.readyState > 0)\
           .sort((a, b) => b.clientWidth * b.clientHeight - a.clientWidth * a.clientHeight);\
         const v = vs.find((v) => !v.paused) ?? vs[0];\
         v ? v.requestPictureInPicture() : console.debug('\\x01rem\\tpip-none'); })()",
    );
}

/// `;s`: save a link through the download handler (~/Downloads).
pub fn download(url: &str) {
    with_active(|b| {
        if let Some(h) = b.host() {
            h.start_download(Some(&url.into()));
        }
    });
}

/// `gF`: DevTools for the active tab, in its own window.
pub fn devtools() {
    with_active(|b| {
        if let Some(h) = b.host() {
            // Ignored for a BrowserView-hosted browser: CEF creates a popup view
            // (see on_popup_browser_view_created).
            h.show_dev_tools(
                Some(&WindowInfo::default()),
                None,
                Some(&BrowserSettings::default()),
                None,
            );
        }
    });
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

/// Height of the active page's view (for page-wise scrolling).
pub fn page_height() -> i32 {
    active().map_or(0, |bv| View::from(&bv).size().height)
}

/// j/k & co.: a mouse-wheel tick in the middle of the page. Chromium scrolls it on the
/// compositor thread with its own smooth-scroll animation (which carries the speed
/// over from tick to tick while a key repeats); no page JavaScript runs.
/// Positive = down / right, in pixels.
pub fn wheel(dx: i32, dy: i32) {
    let Some(bv) = active() else { return };
    let size = View::from(&bv).size();
    let ev = MouseEvent {
        x: size.width / 2,
        y: size.height / 2,
        modifiers: 0,
    };
    if let Some(h) = bv.browser().and_then(|b| b.host()) {
        h.send_mouse_wheel_event(Some(&ev), -dx, -dy);
    }
}

pub fn zoom() -> f64 {
    active()
        .and_then(|bv| bv.browser()?.host())
        .map_or(0.0, |h| h.zoom_level())
}

/// Zoom the current page and remember it for that site (like Chrome's per-site zoom).
pub fn set_zoom(level: f64) {
    with_active(|b| {
        if let Some(h) = b.host() {
            h.set_zoom_level(level);
        }
    });
    let host = host_of(&active_url());
    if !host.is_empty() {
        let mut z = zoom_levels();
        z.retain(|(h, _)| *h != host);
        if level.abs() > 0.01 {
            z.push((host, level));
        }
        let text: String = z.iter().map(|(h, l)| format!("{h}\t{l}\n")).collect();
        let path = zoom_file();
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
        let _ = std::fs::write(path, text);
    }
}

fn zoom_file() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let data = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    std::path::PathBuf::from(data).join("rembrowser/zoom")
}

fn zoom_levels() -> Vec<(String, f64)> {
    std::fs::read_to_string(zoom_file())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (h, z) = l.split_once('\t')?;
            Some((h.to_string(), z.parse().ok()?))
        })
        .collect()
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|r| r.split(['/', '?', '#']).next())
        .unwrap_or("")
        .to_string()
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
    let win = cur();
    with_win(win, |w| w.find.clear());
    refresh_right(win);
    with_active(|b| {
        if let Some(h) = b.host() {
            h.stop_finding(1);
        }
    });
}

pub fn refresh_tabbar(win: u32) {
    let Some((tabs, active)) = with_win(win, |w| (w.tabs.clone(), w.active)) else {
        return;
    };
    let titles: Vec<String> = tabs.iter().map(title_of).collect();
    ui::set_tabs(win, &titles, active);
}

pub fn refresh_tabbars() {
    win_ids().into_iter().for_each(refresh_tabbar);
}

fn refresh_right(win: u32) {
    refresh_tabbar(win);
    let Some((n, active, find, progress)) =
        with_win(win, |w| (w.tabs.len(), w.active, w.find.clone(), w.progress))
    else {
        return;
    };
    let load = if progress < 100 {
        format!(" {progress}%")
    } else {
        String::new()
    };
    ui::set_right(win, format!("{find} [{}/{n}]{load}", active + 1).trim_start());
}

/// The status bar is built after the first page loads: creating its widgets costs
/// ~100 ms of UI thread, which at startup lands exactly when the first page commits
/// (measured: launch→loaded 0.55 s with it up front, 0.45 s without).
fn build_bar(win: u32) {
    let Some(w) = window_of(win) else { return };
    if ui::is_built(win) {
        return;
    }
    ui::build(win);
    ui::recolor();
    popup::init(&w, Owner::Win(win));
    select_in(win, with_win(win, |w| w.active).unwrap_or(0));
}

/// (window, tab index) of a tab's browser.
fn index_of(browser: &mut Browser) -> Option<(u32, usize)> {
    WINS.with_borrow(|w| {
        w.iter().find_map(|w| {
            let i = w
                .tabs
                .iter()
                .position(|bv| bv.browser().is_some_and(|b| b.is_same(Some(browser)) != 0))?;
            Some((w.id, i))
        })
    })
}

/// The window this browser is the visible tab of.
fn shown_in(browser: &mut Browser) -> Option<u32> {
    let (win, i) = index_of(browser)?;
    (with_win(win, |w| w.active) == Some(i)).then_some(win)
}

fn on_tab_closed(browser: &mut Browser) {
    let Some((win, i)) = index_of(browser) else { return };
    let Some((removed, content, n, active)) = with_win(win, |w| {
        (w.tabs.remove(i), w.content.clone(), w.tabs.len(), w.active)
    }) else {
        return;
    };
    let url = url_of(&removed);
    if !url.is_empty() && url != "about:blank" {
        CLOSED.with_borrow_mut(|c| c.push(url));
    }
    content.remove_child_view(Some(&mut View::from(&removed)));
    if n == 0 {
        // Last tab gone: close the window (the last window quits, vimb `:q`).
        // Next tick: we may be inside can_close's loop right now (closing a tab
        // re-entering can_close crashes CEF on the half-closed popup).
        ui::run_on_ui(0, move || close_window(win));
        return;
    }
    ui::reveal_tabs();
    if i < active {
        with_win(win, |w| w.active = active - 1);
        refresh_right(win);
    } else if i == active {
        // Closed the visible tab: show its left neighbour, like vim's bufferline.
        with_win(win, |w| w.active = usize::MAX);
        select_in(win, i.saturating_sub(1).min(n - 1));
    } else {
        refresh_right(win);
    }
}

wrap_window_delegate! {
    struct RemWindowDelegate {
        id: u32,
        url: String,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            Size { width: 1280, height: 800 }
        }

        // Resize / fullscreen: the floating bars follow the window edges.
        fn on_layout_changed(&self, _view: Option<&mut View>, _new_bounds: Option<&Rect>) {
            ui::place();
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
            // [ tab bar ] over [ tabs (flex) ] over [ status bar ]. The bars start as
            // empty fixed-height slots so the page doesn't shift when they fill in.
            let layout = window.set_to_box_layout(Some(&BoxLayoutSettings {
                cross_axis_alignment: AxisAlignment::STRETCH,
                ..Default::default()
            }));
            let id = self.id;
            let (top, status) = ui::frame(id, window);
            let content = panel_create(None).expect("panel");
            content.set_to_fill_layout();
            window.add_child_view(Some(&mut View::from(&top)));
            window.add_child_view(Some(&mut View::from(&content)));
            window.add_child_view(Some(&mut View::from(&status)));
            if let Some(l) = layout {
                l.set_flex_for_view(Some(&mut View::from(&content)), 1);
            }
            View::from(&*window).set_background_color(BG);
            WINS.with_borrow_mut(|w| w.push(Win {
                id,
                window: window.clone(),
                content,
                tabs: Vec::new(),
                active: 0,
                find: String::new(),
                progress: 100,
                confirmed: false,
            }));
            CUR.set(id);
            ui::attach(id);
            if let Some(bv) = new_view(&self.url) {
                add(id, bv, false);
            }
            window.show();
            // Fallback for slow first pages; normally the first load_end builds it.
            ui::run_on_ui(1000, move || build_bar(id));
        }

        fn on_window_activation_changed(&self, _window: Option<&mut Window>, active: i32) {
            if active != 0 {
                set_cur(self.id);
            }
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            WINS.with_borrow_mut(|w| w.retain(|w| w.id != self.id));
            // The bars and popup belonged to this window.
            ui::forget_window(self.id);
            popup::forget(Owner::Win(self.id));
            // Web apps keep running on their own; quit with the last window.
            if cur() == 0 && apps::is_empty() {
                quit_message_loop();
            }
        }

        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            let id = self.id;
            // "Close this window?" first; yes comes back here with `confirmed` set.
            if unconfirmed().contains(&id) {
                ui::run_on_ui(0, move || {
                    // The question shows in the current window: this one.
                    set_cur(id);
                    ask_quit(vec![id]);
                });
                return 0;
            }
            // The last window takes the DevTools windows with it: one still open
            // keeps CEF from shutting down. Each one's on_window_destroyed retries.
            if win_ids() == [id] {
                CLOSING.set(true);
                let devtools = DEVTOOLS.with_borrow(|d| d.clone());
                if !devtools.is_empty() {
                    devtools.iter().for_each(|w| w.close());
                    return 0;
                }
            }
            // Ask every tab (runs beforeunload). Each closed tab hits on_tab_closed;
            // the last one closes the window again, which then passes.
            // The popup overlay too: a browser left open at shutdown trips a CHECK.
            let mut hosts: Vec<_> = with_win(id, |w| w.tabs.iter().filter_map(|bv| bv.browser()?.host()).collect()).unwrap_or_default();
            hosts.extend(popup::host(Owner::Win(id)));
            hosts.iter().fold(1, |all, h| all & h.try_close_browser())
        }

        fn linux_window_properties(
            &self,
            _window: Option<&mut Window>,
            properties: Option<&mut LinuxWindowProperties>,
        ) -> i32 {
            let Some(p) = properties else { return 0 };
            p.wayland_app_id = static_str(APP_ID);
            p.wm_class_class = static_str(APP_ID);
            p.wm_class_name = static_str(APP_ID);
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

        // Popups that become tabs must be Alloy like the others. DevTools is the
        // exception: CEF only supports Chrome style there (default delegate).
        fn delegate_for_popup_browser_view(
            &self,
            _browser_view: Option<&mut BrowserView>,
            _settings: Option<&BrowserSettings>,
            _client: Option<&mut Client>,
            is_devtools: i32,
        ) -> Option<BrowserViewDelegate> {
            (is_devtools == 0).then(RemBrowserViewDelegate::new)
        }

        fn on_popup_browser_view_created(
            &self,
            browser_view: Option<&mut BrowserView>,
            popup_browser_view: Option<&mut BrowserView>,
            is_devtools: i32,
        ) -> i32 {
            // A web app's sign-in popup etc.: another window of that app.
            if let Some(id) = browser_view.as_ref().and_then(|v| v.browser()).and_then(|b| apps::app_id_of(&b)) {
                let Some(bv) = popup_browser_view else { return 0 };
                apps::window(bv.clone(), id);
                return 1;
            }
            // DevTools (gF): its own window, tiled next to the page by niri.
            if is_devtools != 0 {
                let Some(bv) = popup_browser_view else { return 0 };
                let mut d = DevToolsWindowDelegate::new(bv.clone());
                window_create_top_level(Some(&mut d));
                return 1;
            }
            // Popups (target=_blank, window.open) become tabs next to their opener.
            // Keeps window.opener, so OAuth "Sign in with X" popups still work.
            let Some(bv) = popup_browser_view else { return 0 };
            let win = browser_view
                .and_then(|v| v.browser())
                .and_then(|mut b| index_of(&mut b))
                .map_or_else(cur, |(win, _)| win);
            add(win, bv.clone(), NEXT_POPUP_BACKGROUND.replace(false));
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
            if let Some(b) = browser.as_ref().filter(|b| apps::is_app(b)) {
                apps::set_title(b, title);
                return;
            }
            let Some(b) = browser else { return };
            let Some((win, _)) = index_of(b) else { return };
            if let Some(w) = shown_in(b).and_then(window_of) {
                w.set_title(title);
            }
            refresh_tabbar(win);
        }

        // Remembered for :app-install (launcher icon). Prefer png/svg over .ico.
        fn on_favicon_urlchange(&self, browser: Option<&mut Browser>, icon_urls: Option<&mut CefStringList>) {
            let (Some(b), Some(list)) = (browser, icon_urls) else { return };
            let urls: Vec<String> = std::mem::take(list).into_iter().map(|u| u.to_string()).collect();
            let best = urls.iter().find(|u| u.ends_with(".png") || u.ends_with(".svg")).or(urls.first()).cloned();
            if let Some(u) = best {
                FAVICONS.with_borrow_mut(|f| f.insert(b.identifier(), u));
            }
        }

        fn on_address_change(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            let main = frame.is_some_and(|f| f.is_main() != 0);
            if let Some(b) = browser.as_ref().filter(|b| main && apps::is_app(b)) {
                ui::set_url_for(Owner::App(b.identifier()), &url.map(|u| u.to_string()).unwrap_or_default());
                return;
            }
            if let Some(win) = browser.filter(|_| main).and_then(shown_in) {
                ui::set_url_for(Owner::Win(win), &url.map(|u| u.to_string()).unwrap_or_default());
            }
        }

        fn on_loading_progress_change(&self, browser: Option<&mut Browser>, progress: f64) {
            if let Some(win) = browser.and_then(shown_in) {
                with_win(win, |w| w.progress = (progress * 100.0) as u8);
                refresh_right(win);
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
            // Answer to a permission prompt shown inside a web-app window.
            if let Some(rest) = body.strip_prefix("perm-answer\t") {
                if let (Some(b), Some((id, choice))) = (browser.as_ref(), rest.split_once('\t')) {
                    if let Ok(id) = id.parse() {
                        permissions::answer_from(b.identifier(), id, choice);
                    }
                }
                return 1;
            }
            // Homescreen search box: only from the homescreen itself.
            if let Some(text) = body.strip_prefix("home-open\t") {
                let from_home = browser.as_ref().and_then(|b| b.main_frame()).is_some_and(|f| home::is_home(&CefString::from(&f.url()).to_string()));
                if from_home {
                    commands::open(text, false);
                }
                return 1;
            }
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
            if browser.is_some_and(|b| is_current(b)) {
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
        fn on_load_start(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType) {
            let Some(f) = frame.filter(|f| f.is_main() != 0) else { return };
            let url = CefString::from(&f.url()).to_string();
            // This site's remembered zoom.
            let host = host_of(&url);
            if let (Some((_, z)), Some(h)) = (zoom_levels().into_iter().find(|(h, _)| *h == host), browser.and_then(|b| b.host())) {
                h.set_zoom_level(z);
            }
            if !url.starts_with("http") {
                return;
            }
            let (css, generic) = adblock::page_css(&url);
            let js = format!("{PAGE_JS};__rem.cosmetic({},{generic})", popup::js_str(&css));
            f.execute_java_script(Some(&js.as_str().into()), Some(&"rembrowser://page.js".into()), 0);
            if let Some(js) = userscripts::for_url(&url, userscripts::RunAt::Start) {
                f.execute_java_script(Some(&js.as_str().into()), Some(&"rembrowser://userscripts".into()), 0);
            }
        }

        fn on_load_end(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _http_status_code: i32) {
            let Some(f) = frame.filter(|f| f.is_main() != 0) else { return };
            // A web app's command popup, ready before its first `:` (like the browser's).
            if let Some(id) = browser.as_ref().filter(|b| apps::is_app(b)).map(|b| b.identifier()) {
                if let Some(w) = apps::window_of(id) {
                    popup::init(&w, Owner::App(id));
                    ui::app_bar(&w, id);
                }
            }
            // Install page.js early so focus → INSERT shows without a key press.
            f.execute_java_script(Some(&PAGE_JS.into()), Some(&"rembrowser://page.js".into()), 0);
            if let Some(js) = userscripts::for_url(&CefString::from(&f.url()).to_string(), userscripts::RunAt::End) {
                f.execute_java_script(Some(&js.as_str().into()), Some(&"rembrowser://userscripts".into()), 0);
            }
            let url = CefString::from(&f.url()).to_string();
            let title = browser
                .as_ref()
                .and_then(|b| b.host()?.visible_navigation_entry())
                .map(|e| CefString::from(&e.title()).to_string())
                .unwrap_or_default();
            commands::record_history(&url, &title);
            if let Some((win, _)) = browser.and_then(index_of).filter(|(win, _)| !ui::is_built(*win)) {
                ui::run_on_ui(0, move || build_bar(win));
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
            if final_update != 0 && browser.is_some_and(|b| is_current(b)) {
                let text = if count == 0 {
                    "no match".into()
                } else {
                    // Chromium reports ordinal 0 for a fresh (incsearch) session though match 1 is active.
                    format!("{}/{count}", active_match_ordinal.max(1)) };
                popup::set_count(&text);
                let win = cur();
                with_win(win, |w| w.find = text);
                refresh_right(win);
            }
        }
    }
}

wrap_life_span_handler! {
    struct RemLifeSpanHandler;

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: i32,
            target_url: Option<&CefString>,
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
            // Web apps: window.open popups (sign-in) stay with the app; ordinary
            // links go to the browser as tabs.
            if browser.is_some_and(|b| apps::is_app(b)) && target_disposition != WindowOpenDisposition::NEW_POPUP {
                let url = target_url.map(|u| u.to_string()).unwrap_or_default();
                ui::run_on_ui(0, move || open_or_create(&url));
                return 1;
            }
            NEXT_POPUP_BACKGROUND.set(target_disposition == WindowOpenDisposition::NEW_BACKGROUND_TAB);
            0
        }

        fn do_close(&self, browser: Option<&mut Browser>) -> i32 {
            // A web app's browser closing = its window closing (the default).
            if browser.as_ref().is_some_and(|b| apps::is_app(b)) {
                return 0;
            }
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
            browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: Option<&mut sys::XEvent>,
            _is_keyboard_shortcut: Option<&mut i32>,
        ) -> i32 {
            let Some(e) = event else { return 0 };
            // Keys act on the window they came from (a web app or a browser window).
            match browser.as_ref().and_then(|b| apps::view_of(b)) {
                Some(app) => set_app_focus(Some(app)),
                None => set_cur(browser.and_then(index_of).map_or_else(cur, |(win, _)| win)),
            }
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

// DevTools window: just the view, plus an app_id niri rules can match.
wrap_window_delegate! {
    struct DevToolsWindowDelegate {
        view: BrowserView,
    }

    impl ViewDelegate {}

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
        fn on_window_created(&self, window: Option<&mut Window>) {
            let Some(w) = window else { return };
            w.set_to_fill_layout();
            w.add_child_view(Some(&mut View::from(&self.view)));
            w.set_title(Some(&"DevTools — rembrowser".into()));
            w.show();
            DEVTOOLS.with_borrow_mut(|d| d.push(w.clone()));
        }

        fn on_window_destroyed(&self, window: Option<&mut Window>) {
            let Some(w) = window else { return };
            DEVTOOLS.with_borrow_mut(|d| d.retain(|x| View::from(x).is_same(Some(&mut View::from(&*w))) == 0));
            if CLOSING.get() {
                ui::run_on_ui(0, quit_all);
            }
        }

        // Closing the window closes DevTools' browser first (like the main window).
        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            self.view.browser().and_then(|b| b.host()).map_or(1, |h| h.try_close_browser())
        }

        fn linux_window_properties(
            &self,
            _window: Option<&mut Window>,
            properties: Option<&mut LinuxWindowProperties>,
        ) -> i32 {
            let Some(p) = properties else { return 0 };
            p.wayland_app_id = static_str(DEVTOOLS_APP_ID);
            p.wm_class_class = static_str(DEVTOOLS_APP_ID);
            p.wm_class_name = static_str(DEVTOOLS_APP_ID);
            1
        }
    }
}
