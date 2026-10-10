//! Native chrome (CEF Views widgets): tab bar, bookmark bar, status bar. No extra
//! renderer process, no page CSS to fight. Catppuccin Mocha, like the vimb config.
//! The command line is the popup (popup.rs).
//!
//! tab bar:      [ 1 GitHub × ][ 2 Arch Wiki ][ 3 … ]     (pill = active, × closes)
//! bookmark bar: [ Arch Wiki ][ GitHub ][ … ]             (only with `set bookmark-bar`)
//! status bar:   [ MODE ][ url / message                      ][ 3/10 [2/5] 42% ]
//!
//! Zen by default: tab bar and status bar float over the page and only appear when
//! something happens (mode change, message, navigation, tab switch). `set status-bar` /
//! `set tab-bar` pin them. Every browser window has its own set.

use crate::popup::Owner;
use crate::{commands, tabs};
use cef::*;
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const FONT: &str = "CaskaydiaMono Nerd Font Mono, monospace, 15px";
const FONT_BOLD: &str = "CaskaydiaMono Nerd Font Mono, monospace, Bold 15px";
const TAB_FONT: &str = "CaskaydiaMono Nerd Font Mono, monospace, 13px";
const TAB_FONT_BOLD: &str = "CaskaydiaMono Nerd Font Mono, monospace, Bold 13px";
const BAR_BG: u32 = 0xFF18_1825;
const TABBAR_BG: u32 = 0xFF11_111B;
const TEXT: u32 = 0xFFCD_D6F4;
const SUBTEXT: u32 = 0xFFA6_ADC8;
const SURFACE: u32 = 0xFF31_3244;
const OVERLAY: u32 = 0xFF6C_7086;
const GREEN: u32 = 0xFFA6_E3A1;
const RED: u32 = 0xFFF3_8BA8;

/// Mode colours.
pub const BLUE: u32 = 0xFF89_B4FA;
pub const YELLOW: u32 = 0xFFF9_E2AF;
pub const MAUVE: u32 = 0xFFCB_A6F7;
pub const PEACH: u32 = 0xFFFA_B387;

/// Longest tab title shown (chars).
const TAB_TITLE: usize = 22;

/// A status line. A browser window's lives in its Chrome (tab-bar aware); each web app
/// window gets its own overlay.
struct Bar {
    owner: Owner,
    /// Web-app bars: their overlay and window (the browser's are in Chrome).
    app: Option<(OverlayController, Window)>,
    /// Held up: hint / command / pending keys (short-lived states).
    hold: bool,
    mode: LabelButton,
    url: LabelButton,
    right: LabelButton,
    url_text: String,
    mode_text: String,
    mode_color: u32,
}

/// One browser window's bars. They float over the page as overlays, so showing one
/// never resizes the page. Pinned bars (`set status-bar` / `set tab-bar` /
/// `set bookmark-bar`) get a spacer slot of their height in the window layout, which
/// keeps the page clear of them.
struct Chrome {
    win: u32,
    window: Window,
    top_slot: Panel,
    status_slot: Panel,
    tabbar: Panel,
    bookbar: Panel,
    status: Panel,
    tab_ctl: Option<OverlayController>,
    book_ctl: Option<OverlayController>,
    status_ctl: Option<OverlayController>,
}

thread_local! {
    static BARS: RefCell<Vec<Bar>> = const { RefCell::new(Vec::new()) };
    static CHROME: RefCell<Vec<Chrome>> = const { RefCell::new(Vec::new()) };
    /// Slot heights (top, bottom) the window layouts were last told about.
    static SLOTS: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
    /// Unpinned bars stay up until these instants.
    static STATUS_UNTIL: Cell<Option<Instant>> = const { Cell::new(None) };
    static TABS_UNTIL: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Top-right notice (downloads) in the focused window: created on first use.
    static TOAST: RefCell<Option<(Owner, Window, OverlayController, LabelButton)>> = const { RefCell::new(None) };
    static TOAST_UNTIL: Cell<Option<Instant>> = const { Cell::new(None) };
}

const TABBAR_H: i32 = 30;
const BOOKBAR_H: i32 = 26;
const STATUS_H: i32 = 28;
/// Bookmarks shown in the bar.
/// ponytail: the rest is clipped; add an overflow menu if someone keeps more up there.
const BOOKBAR_MAX: usize = 40;
/// How long an unpinned bar stays after something happened.
const SHOW_MS: u64 = 1500;

fn with_owner<R>(o: Owner, f: impl FnOnce(&mut Bar) -> R) -> Option<R> {
    BARS.with_borrow_mut(|b| b.iter_mut().find(|b| b.owner == o).map(f))
}

/// The status line of the focused window (web app or browser).
fn with<R>(f: impl FnOnce(&mut Bar) -> R) -> Option<R> {
    with_owner(tabs::current_owner(), f)
}

fn set_color(l: &LabelButton, color: u32) {
    for state in [
        ButtonState::NORMAL,
        ButtonState::HOVERED,
        ButtonState::PRESSED,
    ] {
        l.set_text_color(state, color);
    }
}

fn pinned(key: &str) -> bool {
    commands::opt_bool(key, false)
}

/// Height the pinned bars need above the page (top) or below it.
fn slot_height(top: bool) -> i32 {
    if top {
        pinned("tab-bar") as i32 * TABBAR_H + pinned("bookmark-bar") as i32 * BOOKBAR_H
    } else {
        pinned("status-bar") as i32 * STATUS_H
    }
}

fn with_chrome<R>(win: u32, f: impl FnOnce(&mut Chrome) -> R) -> Option<R> {
    CHROME.with_borrow_mut(|c| c.iter_mut().find(|c| c.win == win).map(f))
}

fn button(
    text: &str,
    color: u32,
    font: &str,
    press: Press,
    reveal: Option<(LabelButton, u32)>,
) -> LabelButton {
    let mut d = BarButton::new(press, reveal);
    let l = label_button_create(Some(&mut d), Some(&text.into())).expect("label");
    l.set_font_list(Some(&font.into()));
    set_color(&l, color);
    View::from(&l).set_focusable(0);
    l
}

fn label(text: &str, color: u32, font: &str) -> LabelButton {
    button(text, color, font, Press::None, None)
}

fn bar_panel(bg: u32, spacing: i32, insets: Insets, min_h: i32) -> Panel {
    let mut d = Painted::new(bg);
    let p = panel_create(Some(&mut d)).expect("panel");
    p.set_to_box_layout(Some(&BoxLayoutSettings {
        horizontal: 1,
        between_child_spacing: spacing,
        inside_border_insets: insets,
        minimum_cross_axis_size: min_h,
        cross_axis_alignment: AxisAlignment::STRETCH,
        ..Default::default()
    }));
    p
}

/// A browser window closed: forget its widgets.
pub fn forget_window(win: u32) {
    forget(Owner::Win(win));
    CHROME.with_borrow_mut(|c| c.retain(|c| c.win != win));
}

/// That window closed: drop its status line (and the toast if it was there).
pub fn forget(owner: Owner) {
    BARS.with_borrow_mut(|b| b.retain(|b| b.owner != owner));
    TOAST.with_borrow_mut(|t| {
        if t.as_ref().is_some_and(|t| t.0 == owner) {
            t.take();
        }
    });
}

pub fn is_built(win: u32) -> bool {
    with_owner(Owner::Win(win), |_| ()).is_some()
}

/// Some window can show a message (false while the startup config is read).
pub fn is_up() -> bool {
    BARS.with_borrow(|b| !b.is_empty())
}

/// Mode pill, URL/message, right-hand info: the three labels of a status line.
fn labels(panel: &Panel) -> (LabelButton, LabelButton, LabelButton) {
    let mode = label(" NORMAL ", TABBAR_BG, FONT_BOLD);
    let url = label("", TEXT, FONT);
    url.set_horizontal_alignment(HorizontalAlignment::LEFT);
    let right = label("", SUBTEXT, FONT);
    for v in [View::from(&mode), View::from(&url), View::from(&right)] {
        panel.add_child_view(Some(&mut v.clone()));
    }
    if let Some(l) = panel.get_layout().and_then(|l| l.as_box_layout()) {
        l.set_flex_for_view(Some(&mut View::from(&url)), 1);
    }
    (mode, url, right)
}

/// A web app window's own status line (after its first load, like the browser's).
pub fn app_bar(w: &Window, id: i32) {
    let owner = Owner::App(id);
    if with_owner(owner, |_| ()).is_some() {
        return;
    }
    let panel = bar_panel(
        BAR_BG,
        12,
        Insets {
            top: 0,
            left: 0,
            bottom: 0,
            right: 8,
        },
        STATUS_H,
    );
    let (mode, url, right) = labels(&panel);
    let Some(ctl) = w.add_overlay_view(Some(&mut View::from(&panel)), DockingMode::CUSTOM, 0)
    else {
        return;
    };
    ctl.set_visible(0);
    BARS.with_borrow_mut(|b| {
        b.push(Bar {
            owner,
            app: Some((ctl, w.clone())),
            hold: false,
            mode,
            url,
            right,
            url_text: String::new(),
            mode_text: "NORMAL".into(),
            mode_color: BLUE,
        })
    });
    place();
    run_on_ui(0, recolor);
}

/// Keyboard focus moved between windows: show the right status line.
pub fn focus_changed() {
    update();
}

/// (top slot, status bar slot) for a new window's layout: spacers as tall as the
/// pinned bars. Cheap — the labels (~100 ms of UI thread) come in build(), after the
/// first page has loaded.
pub fn frame(win: u32, window: &Window) -> (Panel, Panel) {
    let slot = |top| panel_create(Some(&mut Slot::new(top))).expect("panel");
    let (top_slot, status_slot) = (slot(true), slot(false));
    let tabbar = bar_panel(
        TABBAR_BG,
        0,
        Insets {
            top: 3,
            left: 6,
            bottom: 0,
            right: 6,
        },
        TABBAR_H - 3,
    );
    let bookbar = bar_panel(
        BAR_BG,
        2,
        Insets {
            top: 0,
            left: 6,
            bottom: 0,
            right: 6,
        },
        BOOKBAR_H,
    );
    let status = bar_panel(
        BAR_BG,
        12,
        Insets {
            top: 0,
            left: 0,
            bottom: 0,
            right: 8,
        },
        STATUS_H,
    );
    CHROME.with_borrow_mut(|c| {
        c.push(Chrome {
            win,
            window: window.clone(),
            top_slot: top_slot.clone(),
            status_slot: status_slot.clone(),
            tabbar,
            bookbar,
            status,
            tab_ctl: None,
            book_ctl: None,
            status_ctl: None,
        })
    });
    (top_slot, status_slot)
}

/// Float the bars over the window (after frame()); hidden until needed.
pub fn attach(win: u32) {
    with_chrome(win, |c| {
        let over = |p: &Panel| {
            c.window
                .add_overlay_view(Some(&mut View::from(p)), DockingMode::CUSTOM, 0)
        };
        // The tab bar last: when it pops up unpinned it covers the bookmark bar.
        c.book_ctl = over(&c.bookbar);
        c.tab_ctl = over(&c.tabbar);
        c.status_ctl = over(&c.status);
    });
    place();
    update();
}

/// Overlay bounds follow the window (resize, fullscreen).
pub fn place() {
    BARS.with_borrow(|bars| {
        for (ctl, win) in bars.iter().filter_map(|b| b.app.as_ref()) {
            let size = View::from(win).size();
            ctl.set_bounds(Some(&Rect {
                x: 0,
                y: size.height - STATUS_H,
                width: size.width,
                height: STATUS_H,
            }));
        }
    });
    TOAST.with_borrow(|t| {
        let Some((owner, win, ctl, _)) = t else {
            return;
        };
        let size = View::from(win).size();
        let pref = ctl
            .contents_view()
            .map(|v| v.preferred_size())
            .unwrap_or_default();
        let width = pref.width.min(size.width - 24);
        // Below a pinned tab bar (browser window); 12px from the edges.
        let top = if matches!(owner, Owner::Win(_)) {
            slot_height(true)
        } else {
            0
        } + 12;
        ctl.set_bounds(Some(&Rect {
            x: size.width - width - 12,
            y: top,
            width,
            height: STATUS_H + 4,
        }));
    });
    let tabs_h = pinned("tab-bar") as i32 * TABBAR_H;
    CHROME.with_borrow(|cs| {
        for c in cs {
            let size = View::from(&c.window).size();
            let bar = |ctl: &Option<OverlayController>, y, height| {
                if let Some(ctl) = ctl {
                    ctl.set_bounds(Some(&Rect {
                        x: 0,
                        y,
                        width: size.width,
                        height,
                    }));
                }
            };
            bar(&c.tab_ctl, 0, TABBAR_H);
            bar(&c.book_ctl, tabs_h, BOOKBAR_H);
            bar(&c.status_ctl, size.height - STATUS_H, STATUS_H);
        }
    });
}

/// Top-right notice. `sticky` keeps it up (download running); otherwise it fades
/// after a few seconds.
pub fn toast(text: &str, sticky: bool) {
    let owner = tabs::current_owner();
    // Lives in one window: move it when another window has focus.
    if TOAST.with_borrow(|t| t.as_ref().is_some_and(|t| t.0 != owner)) {
        if let Some((_, _, ctl, _)) = TOAST.with_borrow_mut(Option::take) {
            ctl.destroy();
        }
    }
    if TOAST.with_borrow(|t| t.is_none()) {
        let Some(w) = tabs::current_window() else {
            return;
        };
        let panel = bar_panel(
            SURFACE,
            0,
            Insets {
                top: 0,
                left: 12,
                bottom: 0,
                right: 12,
            },
            STATUS_H + 4,
        );
        let l = label("", TEXT, FONT);
        View::from(&l).set_background_color(SURFACE);
        panel.add_child_view(Some(&mut View::from(&l)));
        let Some(ctl) = w.add_overlay_view(Some(&mut View::from(&panel)), DockingMode::CUSTOM, 0)
        else {
            return;
        };
        TOAST.with_borrow_mut(|t| *t = Some((owner, w.clone(), ctl, l)));
    }
    TOAST.with_borrow(|t| {
        if let Some((_, _, ctl, l)) = t {
            l.set_text(Some(&text.into()));
            set_color(
                l,
                if text.starts_with('✗') {
                    RED
                } else if sticky {
                    TEXT
                } else {
                    GREEN
                },
            );
            ctl.set_visible(1);
        }
    });
    place();
    TOAST_UNTIL.set((!sticky).then(|| Instant::now() + Duration::from_millis(4000)));
    if !sticky {
        run_on_ui(4030, || {
            if TOAST_UNTIL.get().is_some_and(|t| Instant::now() >= t) {
                TOAST.with_borrow(|t| t.as_ref().map(|(_, _, c, _)| c.set_visible(0)));
            }
        });
    }
}

fn live(until: &'static std::thread::LocalKey<Cell<Option<Instant>>>) -> bool {
    until.get().is_some_and(|t| Instant::now() < t)
}

/// Apply pinned / held / recently-revealed state to slots and overlays.
fn update() {
    let (pin_tabs, pin_books, pin_status) =
        (pinned("tab-bar"), pinned("bookmark-bar"), pinned("status-bar"));
    let cur = tabs::current_owner();
    // A web app window has no spacer for a pinned bar: there it only ever pops up.
    let shown = |b: &Bar| {
        (pin_status && b.app.is_none()) || (b.owner == cur && (b.hold || live(&STATUS_UNTIL)))
    };
    let status: Vec<(Owner, bool)> = BARS.with_borrow(|bars| {
        bars.iter()
            .map(|b| {
                let on = shown(b);
                if let Some((ctl, _)) = &b.app {
                    ctl.set_visible(on as i32);
                }
                (b.owner, on)
            })
            .collect()
    });
    // The page is laid out around pinned bars: tell the layouts when that changes.
    let slots = (slot_height(true), slot_height(false));
    let relayout = SLOTS.replace(slots) != slots;
    // Bars that just appeared: they aren't rebuilt while hidden, so catch up.
    let (mut tabs_new, mut books_new) = (Vec::new(), false);
    CHROME.with_borrow(|cs| {
        for c in cs {
            let owner = Owner::Win(c.win);
            let tabs_on = pin_tabs || (owner == cur && live(&TABS_UNTIL));
            let status_on = status
                .iter()
                .find(|(o, _)| *o == owner)
                .map_or(pin_status, |(_, on)| *on);
            View::from(&c.top_slot).set_visible((slots.0 > 0) as i32);
            View::from(&c.status_slot).set_visible((slots.1 > 0) as i32);
            if relayout {
                View::from(&c.top_slot).invalidate_layout();
                View::from(&c.status_slot).invalidate_layout();
                View::from(&c.window).invalidate_layout();
            }
            if let Some(s) = &c.status_ctl {
                s.set_visible(status_on as i32);
            }
            if let Some(b) = &c.book_ctl {
                books_new |= pin_books && b.is_visible() == 0;
                b.set_visible(pin_books as i32);
            }
            if let Some(t) = &c.tab_ctl {
                if tabs_on && t.is_visible() == 0 {
                    tabs_new.push(c.win);
                }
                t.set_visible(tabs_on as i32);
            }
        }
    });
    if relayout {
        place();
    }
    tabs_new.into_iter().for_each(tabs::refresh_tabbar);
    if books_new {
        refresh_bookmarks();
    }
}

fn reveal(until: &'static std::thread::LocalKey<Cell<Option<Instant>>>, ms: u64) {
    until.set(Some(Instant::now() + Duration::from_millis(ms)));
    update();
    run_on_ui(ms as i64 + 30, update);
}

/// A key went into the page's text field: an unpinned status line that is still
/// showing gets out of the way now instead of covering what is being typed.
pub fn typing() {
    if STATUS_UNTIL.take().is_some() {
        update();
    }
}

/// Something tab-related happened (switch, open, close): flash the tab bar.
pub fn reveal_tabs() {
    reveal(&TABS_UNTIL, SHOW_MS);
}

pub fn build(win: u32) {
    let Some(panel) = with_chrome(win, |c| c.status.clone()) else {
        return;
    };
    let owner = Owner::Win(win);
    let (mode, url, right) = labels(&panel);
    BARS.with_borrow_mut(|b| {
        b.retain(|b| b.owner != owner);
        b.push(Bar {
            owner,
            app: None,
            hold: false,
            mode,
            url,
            right,
            url_text: String::new(),
            mode_text: "NORMAL".into(),
            mode_color: BLUE,
        })
    });
}

/// Window theme colours: every panel and label button defaults to these.
pub fn theme(w: &Window) {
    for id in [
        ColorId::COLOR_PRIMARY_BACKGROUND,
        ColorId::COLOR_BUTTON_BACKGROUND,
        ColorId::COLOR_WINDOW_BACKGROUND,
    ] {
        w.set_theme_color(id.get_raw() as i32, BAR_BG);
    }
    run_on_ui(0, recolor);
}

/// Per-view colours; only stick once the view is in the window (see theme()).
pub fn recolor() {
    BARS.with_borrow(|bars| {
        bars.iter()
            .for_each(|b| View::from(&b.mode).set_background_color(b.mode_color))
    });
    tabs::refresh_tabbars();
    refresh_bookmarks();
}

pub fn set_mode(name: &str, color: u32) {
    // INSERT and PASS last as long as you type: a bar held up that long would sit on
    // the page's bottom edge (a chat's message box). They flash like NORMAL does.
    let hold = !matches!(name, "NORMAL" | "INSERT" | "PASS");
    let changed = with(|b| {
        b.hold = hold;
        if b.mode_text == name && b.mode_color == color {
            return false;
        }
        // lualine-style pill: mode colour background, dark text.
        b.mode.set_text(Some(&format!(" {name} ").as_str().into()));
        b.mode_text = name.into();
        b.mode_color = color;
        View::from(&b.mode).set_background_color(color);
        true
    });
    if changed == Some(true) {
        // Back to normal: linger briefly instead of vanishing mid-glance. Into
        // insert / pass: long enough to read which mode it is.
        match name {
            _ if hold => update(),
            "NORMAL" => reveal(&STATUS_UNTIL, 700),
            _ => reveal(&STATUS_UNTIL, SHOW_MS),
        }
    }
}

fn show_url(b: &mut Bar, url: &str) {
    if crate::home::is_home(url) {
        set_color(&b.url, MAUVE);
        b.url.set_text(Some(&"home".into()));
        return;
    }
    let color = if url.starts_with("https://") {
        GREEN
    } else if url.starts_with("http://") {
        RED
    } else {
        TEXT
    };
    set_color(&b.url, color);
    b.url.set_text(Some(&url.into()));
}

/// A window's page URL (the browser's active tab, or a web app's page).
pub fn set_url_for(owner: Owner, url: &str) {
    let changed = with_owner(owner, |b| {
        let changed = b.url_text != url;
        b.url_text = url.to_string();
        show_url(b, url);
        changed
    });
    if changed == Some(true) {
        reveal(&STATUS_UNTIL, SHOW_MS);
    }
}

/// Message in place of the URL until the next key press.
pub fn message(text: &str, error: bool) {
    with(|b| {
        set_color(&b.url, if error { RED } else { YELLOW });
        b.url.set_text(Some(&text.into()));
    });
    reveal(&STATUS_UNTIL, 2500);
}

pub fn clear_message() {
    with(|b| {
        let url = b.url_text.clone();
        show_url(b, &url);
    });
}

/// Tab count / search / loading info — a browser window's status line.
pub fn set_right(win: u32, text: &str) {
    with_owner(Owner::Win(win), |b| b.right.set_text(Some(&text.into())));
}

/// `set status-bar` / `set tab-bar` / `set bookmark-bar` changed: pin or unpin.
pub fn bars_changed() {
    update();
}

fn short(title: &str, max: usize) -> String {
    let t = title.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let cut: String = t.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Rebuild a window's tab bar: per tab a clickable label and a close button (always
/// there on the active tab, on the others only while the mouse is over the tab).
/// ponytail: full rebuild per change; diff the labels if hundreds of tabs ever lag.
pub fn set_tabs(win: u32, titles: &[String], active: usize) {
    let Some((bar, shown, width)) = with_chrome(win, |c| {
        (
            c.tabbar.clone(),
            c.tab_ctl.as_ref().is_some_and(|t| t.is_visible() != 0),
            View::from(&c.window).size().width,
        )
    }) else {
        return;
    };
    if !shown || !is_built(win) {
        return;
    }
    // Titles shrink so every tab stays reachable (~8px a character, 6 for " 1 " + "× ").
    let room = (width - 12) / titles.len().max(1) as i32 / 8 - 6;
    let room = room.clamp(3, TAB_TITLE as i32) as usize;
    bar.remove_all_child_views();
    for (i, t) in titles.iter().enumerate() {
        let on = i == active;
        let bg = if on { SURFACE } else { TABBAR_BG };
        let title = if t.is_empty() { "New tab" } else { t };
        let x = button(
            "× ",
            if on { OVERLAY } else { bg },
            TAB_FONT_BOLD,
            Press::Close(win, i),
            None,
        );
        x.set_text_color(ButtonState::HOVERED, RED);
        x.set_text_color(ButtonState::PRESSED, RED);
        let l = button(
            &format!(" {} {} ", i + 1, short(title, room)),
            if on { TEXT } else { OVERLAY },
            if on { TAB_FONT_BOLD } else { TAB_FONT },
            Press::Tab(win, i),
            (!on).then(|| (x.clone(), bg)),
        );
        l.set_tooltip_text(Some(&title.into()));
        x.set_tooltip_text(Some(&"Close tab".into()));
        for b in [&l, &x] {
            bar.add_child_view(Some(&mut View::from(b)));
            View::from(b).set_background_color(bg);
        }
    }
    View::from(&bar).invalidate_layout();
}

/// Rebuild the bookmark bars (`set bookmark-bar` turned on, or the bookmarks changed).
pub fn refresh_bookmarks() {
    if !pinned("bookmark-bar") {
        return;
    }
    let marks = commands::bookmarks();
    let bars: Vec<Panel> = CHROME.with_borrow(|cs| cs.iter().map(|c| c.bookbar.clone()).collect());
    for bar in bars {
        bar.remove_all_child_views();
        for (url, title, _) in marks.iter().take(BOOKBAR_MAX) {
            let name = if title.is_empty() { url } else { title };
            let l = button(
                &format!(" {} ", short(name, 18)),
                SUBTEXT,
                TAB_FONT,
                Press::Open(url.clone()),
                None,
            );
            l.set_text_color(ButtonState::HOVERED, TEXT);
            l.set_tooltip_text(Some(&url.as_str().into()));
            bar.add_child_view(Some(&mut View::from(&l)));
            View::from(&l).set_background_color(BAR_BG);
        }
        View::from(&bar).invalidate_layout();
    }
}

// A panel with its own background: CEF re-applies theme colours (on add, show, OS
// theme switch) and then calls on_theme_changed — the documented place to override.
wrap_panel_delegate! {
    struct Painted {
        bg: u32,
    }

    impl ViewDelegate {
        fn on_theme_changed(&self, view: Option<&mut View>) {
            if let Some(v) = view {
                v.set_background_color(self.bg);
            }
        }
    }

    impl PanelDelegate {}
}

// The spacer under pinned bars: as tall as they are right now.
wrap_panel_delegate! {
    struct Slot {
        top: bool,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            Size { width: 1, height: slot_height(self.top) }
        }

        fn height_for_width(&self, _view: Option<&mut View>, _width: i32) -> i32 {
            slot_height(self.top)
        }

        fn on_theme_changed(&self, view: Option<&mut View>) {
            if let Some(v) = view {
                v.set_background_color(BAR_BG);
            }
        }
    }

    impl PanelDelegate {}
}

/// What a click on a bar button does.
#[derive(Clone)]
enum Press {
    None,
    /// (window, tab index)
    Tab(u32, usize),
    Close(u32, usize),
    Open(String),
}

wrap_button_delegate! {
    struct BarButton {
        press: Press,
        // A tab label's close button and the colour that hides it: shown while the
        // mouse is over the label (over the button itself, its hover colour shows).
        reveal: Option<(LabelButton, u32)>,
    }

    impl ViewDelegate {}

    impl ButtonDelegate {
        fn on_button_pressed(&self, _button: Option<&mut Button>) {
            // Next tick, not inside the click handler: these rebuild the bar the
            // pressed button lives in.
            match self.press.clone() {
                Press::None => {}
                Press::Tab(win, i) => run_on_ui(0, move || tabs::select_in(win, i)),
                Press::Close(win, i) => run_on_ui(0, move || tabs::close_tab(win, i)),
                Press::Open(url) => run_on_ui(0, move || commands::open(&url, false)),
            }
        }

        fn on_button_state_changed(&self, button: Option<&mut Button>) {
            let (Some((x, hidden)), Some(b)) = (&self.reveal, button) else { return };
            let over = b.state() != ButtonState::NORMAL;
            x.set_text_color(ButtonState::NORMAL, if over { SUBTEXT } else { *hidden });
        }
    }
}

type Job = Box<dyn FnOnce() + Send>;

wrap_task! {
    struct FnTask {
        job: Arc<Mutex<Option<Job>>>,
    }

    impl Task {
        fn execute(&self) {
            if let Some(f) = self.job.lock().ok().and_then(|mut j| j.take()) {
                f();
            }
        }
    }
}

/// Run `f` on the CEF UI thread (from a worker thread), optionally after `delay_ms`.
pub fn run_on_ui(delay_ms: i64, f: impl FnOnce() + Send + 'static) {
    let mut task = FnTask::new(Arc::new(Mutex::new(Some(Box::new(f)))));
    if delay_ms > 0 {
        post_delayed_task(ThreadId::UI, Some(&mut task), delay_ms);
    } else {
        post_task(ThreadId::UI, Some(&mut task));
    }
}
