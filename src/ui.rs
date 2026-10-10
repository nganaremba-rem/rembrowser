//! Native chrome (CEF Views widgets): status bar + tab bar. No extra renderer process,
//! no page CSS to fight. Catppuccin Mocha, like the vimb config.
//! The command line is the popup (popup.rs).
//!
//! tab bar:    [ 1 GitHub ][ 2 Arch Wiki ][ 3 … ]        (pill = active)
//! status bar: [ MODE ][ url / message                        ][ 3/10 [2/5] 42% ]
//!
//! Zen by default: both float over the page and only appear when something happens
//! (mode change, message, navigation, tab switch). `set status-bar` / `set tab-bar`
//! pin them.

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

/// A status line. The browser window's lives in Chrome (tab-bar aware); each web app
/// window gets its own overlay.
struct Bar {
    owner: Owner,
    /// Web-app bars: their overlay and window (the browser's are in Chrome).
    app: Option<(OverlayController, Window)>,
    /// Held up: insert / hint / pass / command / pending keys.
    hold: bool,
    mode: LabelButton,
    url: LabelButton,
    right: LabelButton,
    url_text: String,
    mode_text: String,
    mode_color: u32,
}

/// The bars float over the page as overlays, so showing one never resizes the
/// page. When pinned (`set status-bar` / `set tab-bar`) a same-height spacer slot
/// in the window layout keeps the page clear of it.
struct Chrome {
    tab_slot: Panel,
    status_slot: Panel,
    tabbar: Panel,
    status: Panel,
    tab_ctl: Option<OverlayController>,
    status_ctl: Option<OverlayController>,
}

thread_local! {
    static BARS: RefCell<Vec<Bar>> = const { RefCell::new(Vec::new()) };
    static CHROME: RefCell<Option<Chrome>> = const { RefCell::new(None) };
    /// Unpinned bars stay up until these instants.
    static STATUS_UNTIL: Cell<Option<Instant>> = const { Cell::new(None) };
    static TABS_UNTIL: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Top-right notice (downloads) in the focused window: created on first use.
    static TOAST: RefCell<Option<(Owner, Window, OverlayController, LabelButton)>> = const { RefCell::new(None) };
    static TOAST_UNTIL: Cell<Option<Instant>> = const { Cell::new(None) };
}

const TABBAR_H: i32 = 30;
const STATUS_H: i32 = 28;
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

fn label(text: &str, color: u32, font: &str, on_press: Option<usize>) -> LabelButton {
    let mut d = TabButton::new(on_press.unwrap_or(usize::MAX));
    let l = label_button_create(Some(&mut d), Some(&text.into())).expect("label");
    l.set_font_list(Some(&font.into()));
    set_color(&l, color);
    View::from(&l).set_focusable(0);
    l
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

/// The browser window closed (web apps may live on): forget its widgets.
pub fn reset() {
    forget(Owner::Main);
    CHROME.with_borrow_mut(|c| c.take());
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

pub fn is_built() -> bool {
    with_owner(Owner::Main, |_| ()).is_some()
}

/// Mode pill, URL/message, right-hand info: the three labels of a status line.
fn labels(panel: &Panel) -> (LabelButton, LabelButton, LabelButton) {
    let mode = label(" NORMAL ", TABBAR_BG, FONT_BOLD, None);
    let url = label("", TEXT, FONT, None);
    url.set_horizontal_alignment(HorizontalAlignment::LEFT);
    let right = label("", SUBTEXT, FONT, None);
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

/// (tab bar slot, status bar slot) for the window layout: empty spacers, visible only
/// while that bar is pinned. Cheap — the labels (~100 ms of UI thread) come in
/// build(), after the first page has loaded.
pub fn frame() -> (Panel, Panel) {
    let tab_slot = bar_panel(TABBAR_BG, 0, Insets::default(), TABBAR_H);
    let status_slot = bar_panel(BAR_BG, 0, Insets::default(), STATUS_H);
    let tabbar = bar_panel(
        TABBAR_BG,
        2,
        Insets {
            top: 3,
            left: 6,
            bottom: 0,
            right: 6,
        },
        TABBAR_H - 3,
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
        *c = Some(Chrome {
            tab_slot: tab_slot.clone(),
            status_slot: status_slot.clone(),
            tabbar,
            status,
            tab_ctl: None,
            status_ctl: None,
        })
    });
    (tab_slot, status_slot)
}

/// Float the bars over the window (after frame()); hidden until needed.
pub fn attach(w: &Window) {
    CHROME.with_borrow_mut(|c| {
        let Some(c) = c else { return };
        c.tab_ctl = w.add_overlay_view(Some(&mut View::from(&c.tabbar)), DockingMode::CUSTOM, 0);
        c.status_ctl = w.add_overlay_view(Some(&mut View::from(&c.status)), DockingMode::CUSTOM, 0);
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
        let top = if *owner == Owner::Main && commands::opt_bool("tab-bar", false) {
            TABBAR_H
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
    let Some(w) = tabs::window() else { return };
    let size = View::from(&w).size();
    CHROME.with_borrow(|c| {
        let Some(c) = c else { return };
        if let Some(t) = &c.tab_ctl {
            t.set_bounds(Some(&Rect {
                x: 0,
                y: 0,
                width: size.width,
                height: TABBAR_H,
            }));
        }
        if let Some(s) = &c.status_ctl {
            s.set_bounds(Some(&Rect {
                x: 0,
                y: size.height - STATUS_H,
                width: size.width,
                height: STATUS_H,
            }));
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
        let l = label("", TEXT, FONT, None);
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
    let pin_tabs = commands::opt_bool("tab-bar", false);
    let pin_status = commands::opt_bool("status-bar", false);
    let tabs_on = pin_tabs || live(&TABS_UNTIL);
    let cur = tabs::current_owner();
    let shown = |b: &Bar| pin_status || (b.owner == cur && (b.hold || live(&STATUS_UNTIL)));
    let status_on = with_owner(Owner::Main, |b| shown(b)).unwrap_or(pin_status);
    BARS.with_borrow(|bars| {
        for b in bars {
            if let Some((ctl, _)) = &b.app {
                ctl.set_visible(shown(b) as i32);
            }
        }
    });
    let tabs_was = CHROME.with_borrow(|c| {
        let c = c.as_ref()?;
        View::from(&c.tab_slot).set_visible(pin_tabs as i32);
        View::from(&c.status_slot).set_visible(pin_status as i32);
        if let Some(s) = &c.status_ctl {
            s.set_visible(status_on as i32);
        }
        let t = c.tab_ctl.as_ref()?;
        let was = t.is_visible() != 0;
        t.set_visible(tabs_on as i32);
        Some(was)
    });
    // A hidden tab bar isn't rebuilt on title changes; catch up when it appears.
    if tabs_on && tabs_was == Some(false) {
        tabs::refresh_tabbar();
    }
}

fn reveal(until: &'static std::thread::LocalKey<Cell<Option<Instant>>>, ms: u64) {
    until.set(Some(Instant::now() + Duration::from_millis(ms)));
    update();
    run_on_ui(ms as i64 + 30, update);
}

/// Something tab-related happened (switch, open, close): flash the tab bar.
pub fn reveal_tabs() {
    reveal(&TABS_UNTIL, SHOW_MS);
}

pub fn build() {
    let Some(panel) = CHROME.with_borrow(|c| c.as_ref().map(|c| c.status.clone())) else {
        return;
    };
    let (mode, url, right) = labels(&panel);
    BARS.with_borrow_mut(|b| {
        b.retain(|b| b.owner != Owner::Main);
        b.push(Bar {
            owner: Owner::Main,
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
    tabs::refresh_tabbar();
}

pub fn set_mode(name: &str, color: u32) {
    let hold = name != "NORMAL";
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
        // Back to normal: linger briefly instead of vanishing mid-glance.
        if hold {
            update()
        } else {
            reveal(&STATUS_UNTIL, 700)
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

/// The active tab's URL; a new one (navigation, tab switch) flashes the status bar.
pub fn set_url(url: &str) {
    set_url_for(Owner::Main, url);
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

/// Tab count / search / loading info — the browser window's status line.
pub fn set_right(text: &str) {
    with_owner(Owner::Main, |b| b.right.set_text(Some(&text.into())));
}

/// `set status-bar` / `set tab-bar` changed: pin or unpin.
pub fn set_bar_visible(_on: bool) {
    update();
}

pub fn set_tabbar_visible(_on: bool) {
    update();
}

fn short(title: &str) -> String {
    let t = title.trim();
    if t.chars().count() <= TAB_TITLE {
        return t.to_string();
    }
    let cut: String = t.chars().take(TAB_TITLE - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Rebuild the tab bar: one clickable label per tab, active one bold + underlined.
/// ponytail: full rebuild per change; diff the labels if hundreds of tabs ever lag.
pub fn set_tabs(titles: &[String], active: usize) {
    let Some((bar, shown)) = CHROME.with_borrow(|c| {
        let c = c.as_ref()?;
        Some((
            c.tabbar.clone(),
            c.tab_ctl.as_ref().is_some_and(|t| t.is_visible() != 0),
        ))
    }) else {
        return;
    };
    if !shown || !is_built() {
        return;
    }
    bar.remove_all_child_views();
    for (i, t) in titles.iter().enumerate() {
        let on = i == active;
        let text = format!(
            " {} {} ",
            i + 1,
            short(if t.is_empty() { "New tab" } else { t })
        );
        let l = label(
            &text,
            if on { TEXT } else { OVERLAY },
            if on { TAB_FONT_BOLD } else { TAB_FONT },
            Some(i),
        );
        bar.add_child_view(Some(&mut View::from(&l)));
        View::from(&l).set_background_color(if on { SURFACE } else { TABBAR_BG });
    }
    View::from(&bar).invalidate_layout();
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

// Status-bar labels pass usize::MAX; tab labels pass their index.
wrap_button_delegate! {
    struct TabButton {
        tab: usize,
    }

    impl ViewDelegate {}

    impl ButtonDelegate {
        fn on_button_pressed(&self, _button: Option<&mut Button>) {
            if self.tab != usize::MAX {
                let i = self.tab;
                // Not inside the click handler: select() rebuilds the tab bar (this button).
                run_on_ui(0, move || tabs::select(i));
            }
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
