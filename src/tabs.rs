//! Windows + tabs. Every tab is a CEF BrowserView in its window's content panel (fill
//! layout); only the active one is visible. The bars (ui.rs) float above.
//!
//! All CEF Views calls happen on the UI thread, so tab state lives in thread_locals
//! (no locks). Rule: never call into vim/ui while holding a borrow here.

use crate::popup::Owner;
use crate::{adblock, apps, commands, downloads, home, permissions, popup, session, ui, userscripts, vim};
use cef::*;
use std::cell::{Cell, RefCell};

pub const APP_ID: &str = "rembrowser";
const DEVTOOLS_APP_ID: &str = "rembrowser-devtools";
const CHROME_APP_ID: &str = "rembrowser-chrome";
/// Catppuccin Mocha base: no white flash while a tab loads.
pub const BG: u32 = 0xFF1E_1E2E;
const PAGE_JS: &str = include_str!("page.js");

/// Secret in every page.js message prefix (`\x01rem:<nonce>\t`). A page cannot read it:
/// page.js is injected at load start, before the page's own scripts, and keeps its
/// `send` in a closure. Without it a page could forge hint-pick / paste / focus.
/// ponytail: a page that hooks console.debug before our inline snippets (pip, copy,
/// paste) could capture it; move those into page.js if that ever matters.
pub fn nonce() -> &'static str {
    static N: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    N.get_or_init(|| {
        use std::io::Read;
        let mut b = [0u8; 16];
        let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok();
        // No /dev/urandom is not a real case on Linux; fall back to something unguessable enough.
        if !ok {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            b = (t ^ u128::from(std::process::id())).to_le_bytes();
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    })
}

/// page.js with this process's nonce filled in.
pub fn page_js() -> String {
    PAGE_JS.replace("__NONCE__", nonce())
}

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
}

thread_local! {
    static WINS: RefCell<Vec<Win>> = const { RefCell::new(Vec::new()) };
    /// The browser window that last had focus: commands and new tabs go there.
    static CUR: Cell<u32> = const { Cell::new(0) };
    static NEXT_WIN: Cell<u32> = const { Cell::new(1) };
    /// Disposition seen in on_before_popup, consumed by on_popup_browser_view_created.
    static NEXT_POPUP_BACKGROUND: Cell<bool> = const { Cell::new(false) };
    /// Open DevTools: (id of the inspected browser, the DevTools view).
    static DEVTOOLS: RefCell<Vec<(i32, BrowserView)>> = const { RefCell::new(Vec::new()) };
    /// URLs of closed tabs, for `u` / `U`.
    static CLOSED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static FAVICONS: RefCell<std::collections::HashMap<i32, String>> = RefCell::new(std::collections::HashMap::new());
    /// Site -> its last known icon (also kept on disk), so a tab shows it the moment you
    /// navigate, before the page has even reported its icon URL. FAV_HOST = site each tab shows.
    static HOST_ICON: RefCell<std::collections::HashMap<String, Image>> = RefCell::new(std::collections::HashMap::new());
    static FAV_HOST: RefCell<std::collections::HashMap<i32, String>> = RefCell::new(std::collections::HashMap::new());
    /// Browser id -> when its renderer last died (see on_render_process_terminated).
    static LAST_CRASH: RefCell<std::collections::HashMap<i32, std::time::Instant>> = RefCell::new(std::collections::HashMap::new());
    /// The web-app window with keyboard focus (None = a browser window).
    static APP_FOCUS: RefCell<Option<BrowserView>> = const { RefCell::new(None) };
    /// Open Chrome-style windows (DevTools, chrome://settings, ...): the last browser
    /// window waits for them before closing.
    static CHROME_WINS: RefCell<Vec<Window>> = const { RefCell::new(Vec::new()) };
    /// The last browser window was asked to close (quit_all / WM close).
    /// ponytail: never reset; if a beforeunload cancels the quit, closing DevTools
    /// later re-asks the tabs. Reset on cancel if that ever matters.
    static CLOSING: Cell<bool> = const { Cell::new(false) };
    /// The hidden window extensions see as "current" exists (see ensure_extension_anchor).
    static ANCHOR: Cell<bool> = const { Cell::new(false) };
    /// Sleeping tabs: placeholder view id -> (url, title). See sleep_idle.
    static ASLEEP: RefCell<std::collections::HashMap<i32, (String, String)>> = RefCell::new(std::collections::HashMap::new());
    /// Browser ids whose page asked not to be slept (sound playing / text typed).
    static KEEP: RefCell<std::collections::HashSet<i32>> = RefCell::new(std::collections::HashSet::new());
    /// View id -> since when this tab has been hidden.
    static HIDDEN: RefCell<std::collections::HashMap<i32, std::time::Instant>> = RefCell::new(std::collections::HashMap::new());
    /// Decoded favicon per browser id (drawn in the tab), and for sleeping tabs per
    /// placeholder view id.
    static FAV_IMG: RefCell<std::collections::HashMap<i32, Image>> = RefCell::new(std::collections::HashMap::new());
    static SLEEP_ICON: RefCell<std::collections::HashMap<i32, Image>> = RefCell::new(std::collections::HashMap::new());
    static NEXT_SLEEP_ID: Cell<i32> = const { Cell::new(1_000_000) };
    static SWEEPING: Cell<bool> = const { Cell::new(false) };
}

/// A sleeping tab's real (url, title); None for a live tab.
fn asleep(bv: &BrowserView) -> Option<(String, String)> {
    let id = View::from(bv).id();
    ASLEEP.with_borrow(|m| m.get(&id).cloned())
}

/// Selecting a sleeping tab loads its page again.
fn wake(bv: &BrowserView) {
    let _t = crate::span("tabs::wake");
    let id = View::from(bv).id();
    SLEEP_ICON.with_borrow_mut(|m| m.remove(&id));
    if let Some((url, _)) = ASLEEP.with_borrow_mut(|m| m.remove(&id)) {
        if let Some(f) = bv.browser().and_then(|b| b.main_frame()) {
            f.load_url(Some(&url.as_str().into()));
        }
    }
}

/// Every minute: a tab hidden for `sleep-after` minutes (default 10, 0 = never) gives
/// its renderer back. REMBROWSER_SLEEP_SECS overrides the limit in seconds (tests).
fn sleep_sweep() {
    let secs = std::env::var("REMBROWSER_SLEEP_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(commands::opt_num("sleep-after", 10).max(0) as u64 * 60);
    if secs > 0 {
        sleep_idle(std::time::Duration::from_secs(secs));
    }
    ui::run_on_ui(sweep_every(), sleep_sweep);
}

fn sweep_every() -> i64 {
    if std::env::var_os("REMBROWSER_SLEEP_SECS").is_some() { 2_000 } else { 60_000 }
}

fn sleep_idle(limit: std::time::Duration) {
    let now = std::time::Instant::now();
    for win in win_ids() {
        let tabs: Vec<(BrowserView, bool)> = with_win(win, |w| {
            w.tabs.iter().cloned().enumerate().map(|(i, bv)| (bv, i == w.active)).collect()
        })
        .unwrap_or_default();
        for (bv, active) in tabs {
            // Keyed by browser id: live tabs all share view id 0.
            let Some(b) = bv.browser() else { continue };
            let bid = b.identifier();
            if active {
                HIDDEN.with_borrow_mut(|h| h.remove(&bid));
                continue;
            }
            if asleep(&bv).is_some() {
                continue;
            }
            let since = HIDDEN.with_borrow_mut(|h| *h.entry(bid).or_insert(now));
            let kept = KEEP.with_borrow(|k| k.contains(&bid));
            if now.duration_since(since) >= limit && !kept && b.is_loading() == 0 && url_of(&bv).starts_with("http") {
                sleep_tab(win, &bv);
            }
        }
    }
}

/// Swap the tab's view for a blank one and close the real browser: its renderer (the
/// RAM) goes away, the tab stays in the bar with the old title until selected.
fn sleep_tab(win: u32, old: &BrowserView) {
    let _t = crate::span("tabs::sleep_tab");
    let (url, title) = (url_of(old), title_of(old));
    let Some(fresh) = new_view("about:blank") else { return };
    let sid = NEXT_SLEEP_ID.replace(NEXT_SLEEP_ID.get() + 1);
    let mut view = View::from(&fresh);
    view.set_id(sid);
    view.set_visible(0);
    let Some(content) = with_win(win, |w| w.content.clone()) else { return };
    content.add_child_view(Some(&mut view));
    let swapped = with_win(win, |w| {
        let i = w.tabs.iter().position(|b| View::from(b).is_same(Some(&mut View::from(old))) != 0)?;
        (i != w.active).then(|| w.tabs[i] = fresh.clone())
    })
    .flatten();
    if swapped.is_none() {
        content.remove_child_view(Some(&mut View::from(&fresh)));
        return;
    }
    ASLEEP.with_borrow_mut(|m| m.insert(sid, (url, title)));
    if let Some(img) = icon_of(old) {
        SLEEP_ICON.with_borrow_mut(|m| m.insert(sid, img));
    }
    let host = old.browser().and_then(|b| {
        let id = b.identifier();
        HIDDEN.with_borrow_mut(|h| h.remove(&id));
        FAVICONS.with_borrow_mut(|m| m.remove(&id));
        FAV_IMG.with_borrow_mut(|m| m.remove(&id));
        FAV_HOST.with_borrow_mut(|m| m.remove(&id));
        KEEP.with_borrow_mut(|k| k.remove(&id));
        b.host()
    });
    // No longer in w.tabs: on_tab_closed ignores this browser, so just release it.
    content.remove_child_view(Some(&mut View::from(old)));
    if let Some(h) = host {
        h.close_browser(1);
    }
    refresh_right(win);
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
    let _t = crate::span("tabs::create_window");
    CLOSING.set(false);
    ensure_extension_anchor();
    if !SWEEPING.replace(true) {
        ui::run_on_ui(sweep_every(), sleep_sweep);
    }
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
        vim::new_page();
        ui::focus_changed();
    }
}

/// Browser window `id` has the keyboard now.
fn set_cur(id: u32) {
    set_app_focus(None);
    if CUR.replace(id) != id {
        vim::new_page();
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

/// The window a page lives in: its web-app window, or the browser window of its tab.
pub fn owner_of(browser: &Browser) -> Option<Owner> {
    if apps::is_app(browser) {
        return Some(Owner::App(browser.identifier()));
    }
    index_of(&mut browser.clone()).map(|(win, _)| Owner::Win(win))
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
    let _t = crate::span("tabs::new_view");
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
    let _t = crate::span("tabs::open");
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
    let _t = crate::span("tabs::add");
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
    session_dirty();
}

/// `usize::MAX` = last tab.
pub fn select(i: usize) {
    select_in(cur(), i);
}

pub fn select_in(win: u32, i: usize) {
    let _t = crate::span("tabs::select_in");
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
    // The loading bar follows the tab now shown.
    ui::load_state(win, bv.browser().is_some_and(|b| b.is_loading() != 0));
    if let Some(w) = window_of(win) {
        w.set_title(Some(&title_of(&bv).as_str().into()));
    }
    ui::set_url_for(Owner::Win(win), &url_of(&bv));
    wake(&bv);
    if win == cur() && switched {
        vim::new_page();
    } else if win == cur() {
        vim::reset();
    }
    refresh_right(win);
    session_dirty();
}

/// Relative move, wrapping (gt / gT).
pub fn cycle(delta: isize) {
    let (n, active) = with_win(cur(), |w| (w.tabs.len() as isize, w.active as isize)).unwrap_or((0, 0));
    if n > 0 {
        select((active + delta).rem_euclid(n) as usize);
    }
}

/// `:q` (d, x, Ctrl-w): close the active tab, or the focused web app's page.
pub fn close_active() {
    if APP_FOCUS.with_borrow(|a| a.is_some()) {
        close_active_now();
    } else if let Some(i) = with_win(cur(), |w| w.active) {
        close_tab(cur(), i);
    }
}

/// Close the active page without the last-tab question (`:app` moves it elsewhere).
pub fn close_active_now() {
    with_active(|b| {
        if let Some(h) = b.host() {
            h.close_browser(0);
        }
    });
}

/// Close tab `i` of a window. A window's last tab takes the window with it, and the
/// last window the browser: that asks first (confirm-quit).
pub fn close_tab(win: u32, i: usize) {
    let found = with_win(win, |w| {
        let host = w.tabs.get(i).and_then(|bv| bv.browser()?.host())?;
        Some((host, w.tabs.len() == 1))
    });
    let Some((host, last)) = found.flatten() else {
        return;
    };
    if !last || !commands::opt_bool("confirm-quit", true) {
        host.close_browser(0);
        return;
    }
    let quits = win_ids().len() == 1 && apps::is_empty();
    let (text, yes) = match quits {
        true => ("Close the last tab and quit rembrowser?", "Quit"),
        false => ("Close the last tab and this window?", "Close"),
    };
    popup::confirm(text, yes, move |yes| {
        if yes {
            host.close_browser(0);
        }
    });
}

pub fn close_window(id: u32) {
    if let Some(w) = window_of(id) {
        w.close();
    }
}

fn close_all() {
    win_ids().into_iter().for_each(close_window);
}

thread_local! {
    /// Windows still to rebuild from the saved session (see restore_session).
    static RESTORE: RefCell<Vec<session::Win>> = const { RefCell::new(Vec::new()) };
    static SESSION_PENDING: Cell<bool> = const { Cell::new(false) };
}

/// A tab whose page is not loaded: blank view, the real url/title kept until selected
/// (the same trick as tab sleep, so restoring 30 tabs costs the RAM of one).
fn sleeping_view(url: &str) -> Option<BrowserView> {
    let fresh = new_view("about:blank")?;
    let sid = NEXT_SLEEP_ID.replace(NEXT_SLEEP_ID.get() + 1);
    View::from(&fresh).set_id(sid);
    ASLEEP.with_borrow_mut(|m| m.insert(sid, (url.to_string(), host_of(url))));
    Some(fresh)
}

/// What is worth bringing back: real pages, not the homescreen or error pages.
fn saveable(url: &str) -> bool {
    (url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://"))
        && !home::is_home(url)
        && !url.contains(['\t', '\n'])
}

/// Something about the tabs changed: write the session soon (debounced).
pub fn session_dirty() {
    if !crate::is_private() && !SESSION_PENDING.replace(true) {
        ui::run_on_ui(1500, session_flush);
    }
}

fn session_flush() {
    let _t = crate::span("tabs::session_flush");
    SESSION_PENDING.set(false);
    // During a quit the windows close one by one: saving then would erase the session.
    if QUITTING.get() || CLOSING.get() {
        return;
    }
    let wins: Vec<session::Win> = WINS.with_borrow(|ws| {
        ws.iter()
            .filter_map(|w| {
                let mut urls = Vec::new();
                let mut active = 0;
                for (i, bv) in w.tabs.iter().enumerate() {
                    let u = url_of(bv);
                    if saveable(&u) {
                        if i <= w.active {
                            active = urls.len();
                        }
                        urls.push(u);
                    }
                }
                (!urls.is_empty()).then_some(session::Win { urls, active })
            })
            .collect()
    });
    // Never replace a session with nothing: closing the last tab must keep it.
    if !wins.is_empty() {
        session::save(&wins);
    }
}

/// Reopen the saved windows. False if there is nothing saved.
pub fn restore_session() -> bool {
    let wins = session::load();
    if wins.is_empty() {
        return false;
    }
    RESTORE.with_borrow_mut(|q| q.extend(wins.iter().cloned()));
    for w in &wins {
        create_window(w.urls[0].clone());
    }
    true
}

thread_local! {
    /// `:qa` / Ctrl-q was used: really quit, even with `keep-running` on.
    static QUITTING: Cell<bool> = const { Cell::new(false) };
}

/// Nothing left open (no windows, no web apps): end the process? Not when `keep-running`
/// is on (the next `rembrowser URL` then opens a window at once, no engine start), unless
/// the user asked to quit (`:qa`) or this is a private window (it must leave nothing).
pub fn quit_when_empty() -> bool {
    QUITTING.get() || crate::is_private() || !commands::opt_bool("keep-running", false)
}

/// `:qa` / Ctrl-q: close every browser window, which quits unless web apps are
/// open. Asks first (confirm-quit); closing a window through the window manager
/// does not.
pub fn quit_all() {
    let (wins, tabs) = WINS.with_borrow(|w| (w.len(), w.iter().map(|w| w.tabs.len()).sum::<usize>()));
    if tabs == 0 || !commands::opt_bool("confirm-quit", true) {
        quit_now();
        return;
    }
    let s = |n: usize| if n == 1 { "" } else { "s" };
    let text = format!("Quit rembrowser? ({wins} window{}, {tabs} tab{})", s(wins), s(tabs));
    popup::confirm(&text, "Quit", |yes| {
        if yes {
            quit_now();
        }
    });
}

/// Close every browser window and quit. With no window open (a `keep-running` browser
/// waiting in the background) no "window destroyed" event would come, so quit directly.
fn quit_now() {
    QUITTING.set(true);
    if win_ids().is_empty() && apps::is_empty() {
        quit_message_loop();
    } else {
        close_all();
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
    if let Some((url, _)) = asleep(bv) {
        return url;
    }
    bv.browser()
        .and_then(|b| b.main_frame())
        .map(|f| CefString::from(&f.url()).to_string())
        .unwrap_or_default()
}

fn title_of(bv: &BrowserView) -> String {
    if let Some((_, title)) = asleep(bv) {
        return format!("💤 {title}");
    }
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
            Some(&format!("{};{code}", page_js()).as_str().into()),
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
        &"(() => { if (document.pictureInPictureElement) return document.exitPictureInPicture();\
         const vs = [...document.querySelectorAll('video')].filter((v) => v.readyState > 0)\
           .sort((a, b) => b.clientWidth * b.clientHeight - a.clientWidth * a.clientHeight);\
         const v = vs.find((v) => !v.paused) ?? vs[0];\
         v ? v.requestPictureInPicture() : console.debug('\\x01rem:__NONCE__\\tpip-none'); })()"
            .replace("__NONCE__", nonce()),
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

/// The open DevTools of the browser with this id.
fn devtools_of(id: i32) -> Option<Browser> {
    DEVTOOLS.with_borrow_mut(|d| {
        d.retain(|(_, v)| v.browser().is_some());
        d.iter().find(|(of, _)| *of == id).and_then(|(_, v)| v.browser())
    })
}

/// Switch the DevTools of browser `id` to `panel`, waiting (up to `tries` × 200 ms)
/// for a DevTools that is still opening: CEF sends us no load events for its page.
fn show_panel(id: i32, panel: &'static str, tries: u32) {
    let frame = devtools_of(id).filter(|b| b.is_loading() == 0).and_then(|b| b.main_frame());
    match frame.filter(|f| CefString::from(&f.url()).to_string().starts_with("devtools://")) {
        Some(f) => {
            // The page being loaded is not DevTools being ready: ask until the
            // panel's tab is the selected one (4 s at most).
            let js = format!(
                "(() => {{ let n = 0; const t = setInterval(() => {{\
                   try {{ DevToolsAPI.showPanel('{panel}'); }} catch {{}}\
                   const on = [...document.querySelectorAll('*')].some((e) => e.shadowRoot?.querySelector('#tab-{panel}.selected'));\
                   if (on || ++n > 40) clearInterval(t); }}, 100); }})()"
            );
            f.execute_java_script(Some(&js.as_str().into()), None, 0);
        }
        None if tries > 0 => ui::run_on_ui(200, move || show_panel(id, panel, tries - 1)),
        None => {}
    }
}

/// `gF` / Ctrl-Shift-I: DevTools for the active tab, in its own window. `panel`
/// (Ctrl-Shift-J: "console") is the one to show; None = where DevTools was last.
pub fn devtools(panel: Option<&'static str>) {
    with_active(|b| {
        if let Some(h) = b.host() {
            if let Some(p) = panel {
                let id = b.identifier();
                ui::run_on_ui(0, move || show_panel(id, p, 25));
            }
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
                Some(&format!("{};{code}", page_js()).as_str().into()),
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
        ZOOM.with_borrow_mut(|c| *c = Some(z));
    }
}

thread_local! {
    /// The per-site zoom file, read once. ponytail: edits made to the file while the
    /// browser runs are not seen until restart; stat the mtime if that ever matters.
    static ZOOM: RefCell<Option<Vec<(String, f64)>>> = const { RefCell::new(None) };
}

fn zoom_file() -> std::path::PathBuf {
    std::path::PathBuf::from(crate::data_dir()).join("zoom")
}

fn zoom_levels() -> Vec<(String, f64)> {
    ZOOM.with_borrow_mut(|c| {
        c.get_or_insert_with(|| {
            std::fs::read_to_string(zoom_file())
                .unwrap_or_default()
                .lines()
                .filter_map(|l| {
                    let (h, z) = l.split_once('\t')?;
                    Some((h.to_string(), z.parse().ok()?))
                })
                .collect()
        })
        .clone()
    })
}

/// Disk file for a site's icon; None if the host has odd characters (never a path escape).
fn icon_file(host: &str) -> Option<std::path::PathBuf> {
    let ok = !host.is_empty()
        && host.len() < 120
        && host.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-');
    ok.then(|| std::path::Path::new(&crate::data_dir()).join("favicons").join(format!("{host}.png")))
}

fn cached_icon(host: &str) -> Option<Image> {
    if let Some(img) = HOST_ICON.with_borrow(|m| m.get(host).cloned()) {
        return Some(img);
    }
    let bytes = std::fs::read(icon_file(host)?).ok()?;
    let img = cef::image_create()?;
    (img.add_png(1.0, Some(&bytes)) != 0).then_some(())?;
    HOST_ICON.with_borrow_mut(|m| m.insert(host.to_string(), img.clone()));
    Some(img)
}

fn remember_icon(host: &str, img: &Image) {
    HOST_ICON.with_borrow_mut(|m| m.insert(host.to_string(), img.clone()));
    // CEF writes the pixel size through these pointers, so they must not be null.
    let (mut w, mut h) = (0, 0);
    let png = img.as_png(1.0, 1, Some(&mut w), Some(&mut h));
    let (Some(path), Some(png)) = (icon_file(host), png) else { return };
    let (p, n) = (png.raw_data() as *const u8, png.size());
    if p.is_null() || n == 0 {
        return;
    }
    // SAFETY: CEF guarantees raw_data() points at size() bytes while `png` is alive.
    let bytes = unsafe { std::slice::from_raw_parts(p, n) };
    if let Some(dir) = path.parent() {
        use std::os::unix::fs::DirBuilderExt;
        // 0700: file names are the sites you visit.
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir);
    }
    let _ = std::fs::write(path, bytes);
}

/// Dark icons (GitHub, Notion, ...) vanish on the dark tab bar, so put them on a light
/// rounded tile. Bright icons are returned as is. Running it twice changes nothing,
/// because the tiled result is bright.
fn with_chip(img: &Image) -> Image {
    let (mut w, mut h) = (0, 0);
    let Some(bin) = img.as_bitmap(1.0, cef::ColorType::BGRA_8888, cef::AlphaType::POSTMULTIPLIED, Some(&mut w), Some(&mut h))
    else { return img.clone() };
    let n = bin.size();
    if w <= 0 || h <= 0 || n != (w * h * 4) as usize || bin.raw_data().is_null() {
        return img.clone();
    }
    // SAFETY: raw_data() points at size() bytes while `bin` is alive.
    let px = unsafe { std::slice::from_raw_parts(bin.raw_data() as *const u8, n) };
    let (mut sum, mut cnt) = (0.0f32, 0u32);
    for p in px.chunks_exact(4).filter(|p| p[3] > 128) {
        sum += 0.2126 * p[2] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[0] as f32;
        cnt += 1;
    }
    if cnt == 0 || sum / cnt as f32 / 255.0 >= 0.35 {
        return img.clone();
    }
    let (wf, hf, r) = (w as f32, h as f32, (w.min(h) as f32 / 4.0).max(2.0));
    let mut out = Vec::with_capacity(n);
    for (i, p) in px.chunks_exact(4).enumerate() {
        let (x, y) = ((i as i32 % w) as f32 + 0.5, (i as i32 / w) as f32 + 0.5);
        // Inside the rounded square? (distance to the nearest corner centre when in a corner)
        let (dx, dy) = ((x - r).min(wf - r - x).min(0.0), (y - r).min(hf - r - y).min(0.0));
        if dx * dx + dy * dy > r * r {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let a = p[3] as f32 / 255.0;
        let bg = [0xF4u8 as f32, 0xEE as f32, 0xEE as f32]; // B, G, R of #EEEEF4
        for c in 0..3 {
            out.push((p[c] as f32 * a + bg[c] * (1.0 - a)) as u8);
        }
        out.push(255);
    }
    let Some(new) = cef::image_create() else { return img.clone() };
    if new.add_bitmap(1.0, w, h, cef::ColorType::BGRA_8888, cef::AlphaType::POSTMULTIPLIED, Some(&out)) == 0 {
        return img.clone();
    }
    new
}

/// Navigation started in a tab: paint the site's remembered icon now (or none, if unknown).
fn show_cached_icon(id: i32, url: &str) {
    let host = host_of(url);
    if host.is_empty() || FAV_HOST.with_borrow(|m| m.get(&id) == Some(&host)) {
        return;
    }
    match cached_icon(&host) {
        Some(img) => FAV_IMG.with_borrow_mut(|m| m.insert(id, img)),
        None => FAV_IMG.with_borrow_mut(|m| m.remove(&id)),
    };
    FAV_HOST.with_borrow_mut(|m| m.insert(id, host));
    if let Some(win) = win_of_browser(id) {
        refresh_tabbar(win);
    }
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

/// The favicon to draw in this tab (a sleeping tab keeps its own).
fn icon_of(bv: &BrowserView) -> Option<Image> {
    if asleep(bv).is_some() {
        let id = View::from(bv).id();
        return SLEEP_ICON.with_borrow(|m| m.get(&id).cloned());
    }
    let id = bv.browser()?.identifier();
    FAV_IMG.with_borrow(|m| m.get(&id).cloned())
}

thread_local! {
    /// A tab bar redraw is already on its way (see refresh_tabbar).
    static TABBAR_DIRTY: Cell<bool> = const { Cell::new(false) };
}

/// Something a tab bar shows changed (title, icon, active tab, count). The bars are
/// redrawn once, on the next UI tick, however often this is called: a loading page
/// changes its title, icon and progress in bursts.
pub fn refresh_tabbar(_win: u32) {
    refresh_tabbars();
}

pub fn refresh_tabbars() {
    if !TABBAR_DIRTY.replace(true) {
        ui::run_on_ui(0, flush_tabbars);
    }
}

fn flush_tabbars() {
    let _t = crate::span("tabs::flush_tabbars");
    TABBAR_DIRTY.set(false);
    let mut more = false;
    for win in win_ids() {
        let Some((tabs, active)) = with_win(win, |w| (w.tabs.clone(), w.active)) else {
            continue;
        };
        let titles: Vec<String> = tabs.iter().map(title_of).collect();
        let icons: Vec<Option<Image>> = tabs.iter().map(icon_of).collect();
        more |= ui::set_tabs(win, &titles, &icons, active);
    }
    // Buttons still to make (one per tick, they are slow to build): come back shortly.
    if more && !TABBAR_DIRTY.replace(true) {
        ui::run_on_ui(20, flush_tabbars);
    }
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
    let _t = crate::span("tabs::build_bar");
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
    let _t = crate::span("tabs::on_tab_closed");
    let Some((win, i)) = index_of(browser) else { return };
    let Some((removed, content, n, active)) = with_win(win, |w| {
        (w.tabs.remove(i), w.content.clone(), w.tabs.len(), w.active)
    }) else {
        return;
    };
    let url = url_of(&removed);
    if !url.is_empty() && url != "about:blank" {
        // Capped: `u` only ever needs the last few.
        CLOSED.with_borrow_mut(|c| {
            c.push(url);
            if c.len() > 100 {
                c.remove(0);
            }
        });
    }
    // Per-tab state keyed by browser id must not outlive the tab.
    let vid = View::from(&removed).id();
    ASLEEP.with_borrow_mut(|m| m.remove(&vid));
    SLEEP_ICON.with_borrow_mut(|m| m.remove(&vid));
    let id = browser.identifier();
    FAV_IMG.with_borrow_mut(|m| m.remove(&id));
    HIDDEN.with_borrow_mut(|h| h.remove(&id));
    KEEP.with_borrow_mut(|k| k.remove(&id));
    FAVICONS.with_borrow_mut(|m| m.remove(&id));
    FAV_HOST.with_borrow_mut(|m| m.remove(&id));
    LAST_CRASH.with_borrow_mut(|m| m.remove(&id));
    content.remove_child_view(Some(&mut View::from(&removed)));
    session_dirty();
    if n == 0 {
        // Last tab gone: close the window (the last window quits).
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
            let _t = crate::span("cb::on_layout_changed");
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
            let _t = crate::span("cb::on_theme_colors_changed");
            if let Some(w) = window {
                ui::theme(w);
            }
        }

        fn window_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::ALLOY
        }

        fn on_window_created(&self, window: Option<&mut Window>) {
            let _t = crate::span("cb::on_window_created");
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
            }));
            CUR.set(id);
            ui::attach(id);
            if let Some(bv) = new_view(&self.url) {
                add(id, bv, false);
            }
            // A restored session: this window's other tabs, asleep until selected.
            let restore = RESTORE.with_borrow_mut(|q| (!q.is_empty()).then(|| q.remove(0)));
            if let Some(r) = restore {
                for u in r.urls.iter().skip(1) {
                    if let Some(bv) = sleeping_view(u) {
                        add(id, bv, true);
                    }
                }
                if r.active > 0 {
                    select_in(id, r.active);
                }
            }
            crate::mark("window created, shown");
            window.show();
            // Fallback for slow first pages; normally the first load_end builds it.
            ui::run_on_ui(1000, move || build_bar(id));
        }

        fn on_window_activation_changed(&self, _window: Option<&mut Window>, active: i32) {
            let _t = crate::span("cb::on_window_activation_changed");
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
            if cur() == 0 && apps::is_empty() && quit_when_empty() {
                quit_message_loop();
            }
        }

        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            let id = self.id;
            // The last window takes the Chrome-style windows (DevTools, settings) with
            // it: one still open keeps CEF from shutting down. Each one's
            // on_window_destroyed retries.
            if win_ids() == [id] {
                CLOSING.set(true);
                let chrome = CHROME_WINS.with_borrow(|d| d.clone());
                if !chrome.is_empty() {
                    chrome.iter().for_each(|w| w.close());
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
                if let Some(of) = browser_view.as_ref().and_then(|v| v.browser()) {
                    DEVTOOLS.with_borrow_mut(|d| d.push((of.identifier(), bv.clone())));
                }
                let mut d = ChromeWindowDelegate::new(bv.clone(), DEVTOOLS_APP_ID.into(), "DevTools — rembrowser".into(), false, None);
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
            let _t = crate::span("cb::on_title_change");
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
            let _t = crate::span("cb::on_favicon_urlchange");
            let (Some(b), Some(list)) = (browser, icon_urls) else { return };
            let urls: Vec<String> = std::mem::take(list).into_iter().map(|u| u.to_string()).collect();
            let best = urls.iter().find(|u| u.ends_with(".png") || u.ends_with(".svg")).or(urls.first()).cloned();
            let id = b.identifier();
            let Some(u) = best else {
                // A page with no icon: drop the old one. (A reload does not report again, so
                // this is the only place an icon is cleared.)
                FAV_IMG.with_borrow_mut(|m| m.remove(&id));
                if let Some(win) = win_of_browser(id) {
                    refresh_tabbar(win);
                }
                return;
            };
            FAVICONS.with_borrow_mut(|f| f.insert(id, u.clone()));
            // Decoded 16 px copy for the tab (CEF fetches it itself, favicon cache, no cookies).
            if let Some(h) = b.host() {
                let host = FAV_HOST.with_borrow(|m| m.get(&id).cloned()).unwrap_or_default();
                let mut cb = FavCallback::new(id, host);
                h.download_image(Some(&u.as_str().into()), 1, 16, 0, Some(&mut cb));
            }
        }

        fn on_address_change(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            let _t = crate::span("cb::on_address_change");
            let main = frame.is_some_and(|f| f.is_main() != 0);
            if let (true, Some(b), Some(u)) = (main, browser.as_ref(), url) {
                show_cached_icon(b.identifier(), &u.to_string());
            }
            if let Some(b) = browser.as_ref().filter(|b| main && apps::is_app(b)) {
                ui::set_url_for(Owner::App(b.identifier()), &url.map(|u| u.to_string()).unwrap_or_default());
                return;
            }
            if let Some(win) = browser.filter(|_| main).and_then(shown_in) {
                ui::set_url_for(Owner::Win(win), &url.map(|u| u.to_string()).unwrap_or_default());
            }
        }

        fn on_loading_progress_change(&self, browser: Option<&mut Browser>, progress: f64) {
            let _t = crate::span("cb::on_loading_progress_change");
            if let Some(win) = browser.and_then(shown_in) {
                with_win(win, |w| w.progress = (progress * 100.0) as u8);
                ui::load_progress(win, progress as f32);
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
            // Homescreen search box (a static page of ours, no nonce): only from the homescreen itself.
            if let Some(text) = msg.strip_prefix("\u{1}rem\thome-open\t") {
                let from_home = browser.as_ref().and_then(|b| b.main_frame()).is_some_and(|f| home::is_home(&CefString::from(&f.url()).to_string()));
                if from_home {
                    commands::open(text, false);
                }
                return 1;
            }
            // Everything else must carry this process's nonce: a page can't forge it.
            let Some(body) = msg.strip_prefix(&format!("\u{1}rem:{}\t", nonce())) else { return 0 };
            // Tab sleep: this page is playing sound / has typed text, keep it awake.
            if let Some(v) = body.strip_prefix("keep\t") {
                if let Some(b) = browser.as_ref() {
                    let id = b.identifier();
                    KEEP.with_borrow_mut(|k| if v == "1" { k.insert(id); } else { k.remove(&id); });
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

fn win_of_browser(id: i32) -> Option<u32> {
    WINS.with_borrow(|ws| {
        ws.iter()
            .find(|w| w.tabs.iter().any(|bv| bv.browser().is_some_and(|b| b.identifier() == id)))
            .map(|w| w.id)
    })
}

wrap_download_image_callback! {
    struct FavCallback {
        id: i32,
        host: String,
    }

    impl DownloadImageCallback {
        fn on_download_image_finished(&self, _url: Option<&CefString>, status: i32, image: Option<&mut Image>) {
            let _t = crate::span("cb::on_download_image_finished");
            // Tab may be gone by now; then there is no window and nothing to keep.
            let Some(win) = win_of_browser(self.id) else { return };
            match image {
                Some(img) if (200..300).contains(&status) && img.is_empty() == 0 => {
                    let img = with_chip(img);
                    FAV_IMG.with_borrow_mut(|m| m.insert(self.id, img.clone()));
                    remember_icon(&self.host, &img);
                }
                _ => return,
            }
            refresh_tabbar(win);
        }
    }
}

wrap_load_handler! {
    struct RemLoadHandler;

    impl LoadHandler {
        // Cosmetic adblock: site CSS before the page paints; page.js starts reporting
        // class/id names for the generic rules.
        fn on_load_start(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition_type: TransitionType) {
            let _t = crate::span("cb::on_load_start");
            let Some(f) = frame.filter(|f| f.is_main() != 0) else { return };
            crate::mark("load start");
            let url = CefString::from(&f.url()).to_string();
            // A new page starts with no keep-awake claim (its page.js reports afresh).
            if let Some(b) = browser.as_ref() {
                KEEP.with_borrow_mut(|k| k.remove(&b.identifier()));
            }
            // A new page: a field it focuses by itself means insert mode again.
            let mut browser = browser;
            if browser.as_deref_mut().is_some_and(is_current) {
                vim::new_page();
            }
            // This site's remembered zoom.
            let host = host_of(&url);
            if let (Some((_, z)), Some(h)) = (zoom_levels().into_iter().find(|(h, _)| *h == host), browser.and_then(|b| b.host())) {
                h.set_zoom_level(z);
            }
            if !url.starts_with("http") {
                return;
            }
            let (css, generic) = adblock::page_css(&url);
            let js = format!("{};__rem.cosmetic({},{generic})", page_js(), popup::js_str(&css));
            f.execute_java_script(Some(&js.as_str().into()), Some(&"rembrowser://page.js".into()), 0);
            if let Some(js) = userscripts::for_url(&url, userscripts::RunAt::Start) {
                f.execute_java_script(Some(&js.as_str().into()), Some(&"rembrowser://userscripts".into()), 0);
            }
        }

        // Top-edge loading bar of the visible tab.
        fn on_loading_state_change(&self, browser: Option<&mut Browser>, is_loading: i32, _can_go_back: i32, _can_go_forward: i32) {
            let _t = crate::span("cb::on_loading_state_change");
            if let Some(win) = browser.and_then(shown_in) {
                ui::load_state(win, is_loading != 0);
                popup::refocus(Owner::Win(win));
            }
        }

        fn on_load_end(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _http_status_code: i32) {
            let _t = crate::span("cb::on_load_end");
            let Some(f) = frame.filter(|f| f.is_main() != 0) else { return };
            crate::mark("load end");
            let mut browser = browser;
            if let Some(win) = browser.as_deref_mut().and_then(shown_in) {
                popup::refocus(Owner::Win(win));
            }
            // A web app's command popup, ready before its first `:` (like the browser's).
            if let Some(id) = browser.as_ref().filter(|b| apps::is_app(b)).map(|b| b.identifier()) {
                if let Some(w) = apps::window_of(id) {
                    popup::init(&w, Owner::App(id));
                    ui::app_bar(&w, id);
                }
            }
            // Install page.js early so focus → INSERT shows without a key press.
            f.execute_java_script(Some(&page_js().as_str().into()), Some(&"rembrowser://page.js".into()), 0);
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
            session_dirty();
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
            let _t = crate::span("cb::do_close");
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

        // The page's renderer died (crash, out of memory, killed). Without this the tab
        // stays blank and nothing says why. Log it and reload, at most once per 10 s per
        // tab so a page that crashes while loading cannot loop.
        fn on_render_process_terminated(
            &self,
            browser: Option<&mut Browser>,
            status: TerminationStatus,
            error_code: i32,
            _error_string: Option<&CefString>,
        ) {
            let Some(b) = browser else { return };
            let id = b.identifier();
            let url = b.main_frame().map(|f| CefString::from(&f.url()).to_string()).unwrap_or_default();
            eprintln!("rembrowser: renderer gone (status {status:?}, code {error_code}) tab {id}: {url}");
            let again = LAST_CRASH.with_borrow_mut(|m| {
                let now = std::time::Instant::now();
                let recent = m.get(&id).is_some_and(|t| now.duration_since(*t).as_secs() < 10);
                m.insert(id, now);
                !recent
            });
            if again {
                b.reload();
            } else {
                ui::toast("this page keeps crashing: not reloading", true);
            }
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

/// What only Chromium's own UI layer can show: these open in a Chrome-style window
/// (`:settings`, `:extensions`, `:chrome URL`) instead of a tab.
pub fn needs_chrome(url: &str) -> bool {
    ["chrome://settings", "chrome://extensions", "chrome://password-manager"]
        .iter()
        .any(|p| url.starts_with(p))
}

/// A window with Chromium's own UI layer around `url`: settings and extension pages
/// only exist there (the tabs are Alloy style: no Chrome UI, much lighter). One page
/// per window is CEF's limit for this style. `toolbar` adds Chrome's address bar
/// with the extension buttons and menu.
pub fn chrome_window(url: &str, toolbar: bool) {
    make_chrome_window(url, toolbar, false, None);
}

/// An extension's popup page: a small fixed-size window (niri floats those), the
/// nearest thing to the bubble under a toolbar button.
pub fn extension_popup(url: &str) {
    make_chrome_window(url, false, false, Some(Size { width: 400, height: 600 }));
}

/// Extensions ask for "the current window" (`chrome.windows.getCurrent`) before they
/// open a popout; the Alloy tabs are none, so with no Chrome-style window that call
/// fails and, e.g., Bitwarden's "Unlock account" silently does nothing. One window that
/// is never shown fixes it for every extension. Closed with the last browser window
/// like DevTools; the next window made brings it back.
pub fn ensure_extension_anchor() {
    if !commands::opt_bool("extensions", false) || ANCHOR.replace(true) {
        return;
    }
    make_chrome_window("about:blank", false, true, None);
}

fn make_chrome_window(url: &str, toolbar: bool, hidden: bool, size: Option<Size>) {
    let mut client = ChromeClient::new();
    let mut delegate = ChromeViewDelegate::new(toolbar);
    let view = browser_view_create(
        Some(&mut client),
        Some(&url.into()),
        Some(&BrowserSettings::default()),
        None,
        None,
        Some(&mut delegate),
    );
    if let Some(bv) = view {
        let mut d = ChromeWindowDelegate::new(bv, CHROME_APP_ID.into(), "Chromium — rembrowser".into(), hidden, size);
        window_create_top_level(Some(&mut d));
    }
}

wrap_client! {
    struct ChromeClient;

    impl Client {}
}

wrap_browser_view_delegate! {
    struct ChromeViewDelegate {
        toolbar: bool,
    }

    impl ViewDelegate {}

    impl BrowserViewDelegate {
        fn browser_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::CHROME
        }

        fn chrome_toolbar_type(&self, _browser_view: Option<&mut BrowserView>) -> ChromeToolbarType {
            if self.toolbar { ChromeToolbarType::NORMAL } else { ChromeToolbarType::NONE }
        }

        // Chrome's toolbar is complete (has a size) once its browser exists: it
        // goes above the page then.
        fn on_browser_created(&self, browser_view: Option<&mut BrowserView>, _browser: Option<&mut Browser>) {
            let Some(bv) = browser_view else { return };
            if let (Some(mut toolbar), Some(w)) = (bv.chrome_toolbar(), View::from(&*bv).window()) {
                w.add_child_view_at(Some(&mut toolbar), 0);
                View::from(&w).invalidate_layout();
            }
        }
    }
}

// A Chrome-style window (DevTools, settings, extensions): Chrome's toolbar if the view
// has one, the view, and an app_id niri rules can match.
wrap_window_delegate! {
    struct ChromeWindowDelegate {
        view: BrowserView,
        app_id: String,
        title: String,
        hidden: bool,
        size: Option<Size>,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            self.size.clone().unwrap_or(Size { width: 1100, height: 800 })
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        // cef-rs defaults these to 0 (CEF's C++ defaults are true): a fixed-size
        // window, which niri opens floating.
        fn can_resize(&self, _window: Option<&mut Window>) -> i32 {
            self.size.is_none() as i32
        }

        fn can_maximize(&self, _window: Option<&mut Window>) -> i32 {
            self.size.is_none() as i32
        }

        fn can_minimize(&self, _window: Option<&mut Window>) -> i32 {
            1
        }
        fn on_window_created(&self, window: Option<&mut Window>) {
            let _t = crate::span("cb::on_window_created");
            let Some(w) = window else { return };
            let layout = w.set_to_box_layout(Some(&BoxLayoutSettings {
                cross_axis_alignment: AxisAlignment::STRETCH,
                ..Default::default()
            }));
            w.add_child_view(Some(&mut View::from(&self.view)));
            if let Some(l) = layout {
                l.set_flex_for_view(Some(&mut View::from(&self.view)), 1);
            }
            w.set_title(Some(&self.title.as_str().into()));
            if !self.hidden {
                w.show();
                self.view.request_focus();
            }
            CHROME_WINS.with_borrow_mut(|d| d.push(w.clone()));
        }

        fn on_window_destroyed(&self, window: Option<&mut Window>) {
            let Some(w) = window else { return };
            CHROME_WINS.with_borrow_mut(|d| d.retain(|x| View::from(x).is_same(Some(&mut View::from(&*w))) == 0));
            if self.hidden {
                ANCHOR.set(false);
            }
            if CLOSING.get() {
                ui::run_on_ui(0, close_all);
            }
        }

        // Closing the window closes its browser first (like a browser window).
        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            self.view.browser().and_then(|b| b.host()).map_or(1, |h| h.try_close_browser())
        }

        fn linux_window_properties(
            &self,
            _window: Option<&mut Window>,
            properties: Option<&mut LinuxWindowProperties>,
        ) -> i32 {
            let Some(p) = properties else { return 0 };
            p.wayland_app_id = static_str(&self.app_id);
            p.wm_class_class = static_str(&self.app_id);
            p.wm_class_name = static_str(&self.app_id);
            1
        }
    }
}
