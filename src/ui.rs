//! Native chrome (CEF Views widgets): status bar + tab bar. No extra renderer process,
//! no page CSS to fight. Catppuccin Mocha, like the vimb config.
//! The command line is the popup (popup.rs).
//!
//! tab bar:    [ 1 GitHub ][ 2 Arch Wiki ][ 3 … ]        (accent underline = active)
//! status bar: [ MODE ][ url / message                        ][ 3/10 [2/5] 42% ]

use crate::tabs;
use cef::*;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

pub const FONT: &str = "CaskaydiaMono Nerd Font Mono, 15px";
const FONT_BOLD: &str = "CaskaydiaMono Nerd Font Mono, Bold 15px";
const TAB_FONT: &str = "CaskaydiaMono Nerd Font Mono, 13px";
const TAB_FONT_BOLD: &str = "CaskaydiaMono Nerd Font Mono, Bold 13px";
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

struct Bar {
    mode: LabelButton,
    url: LabelButton,
    right: LabelButton,
    url_text: String,
    tabbar: Panel,
    mode_color: u32,
}

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
    /// (tab bar, status bar) slots, created with the window; filled by build().
    static FRAME: RefCell<Option<(Panel, Panel)>> = const { RefCell::new(None) };
}

/// Minimum bar heights: the empty slots made at window creation are already as tall
/// as the filled bars (labels are 27-28px), so filling them shifts nothing.
const TABBAR_H: i32 = 30;
const STATUS_H: i32 = 28;

fn with<R>(f: impl FnOnce(&mut Bar) -> R) -> Option<R> {
    BAR.with_borrow_mut(|b| b.as_mut().map(f))
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

pub fn is_built() -> bool {
    BAR.with_borrow(|b| b.is_some())
}

/// Empty (tab bar, status bar) slots for the window layout. Cheap: no labels, no
/// fonts. The labels cost ~100 ms of UI thread, so build() fills them in after
/// the first page has loaded.
pub fn frame() -> (Panel, Panel) {
    let mut d = Painted::new(TABBAR_BG);
    let tabbar = panel_create(Some(&mut d)).expect("panel");
    tabbar.set_to_box_layout(Some(&BoxLayoutSettings {
        horizontal: 1,
        between_child_spacing: 2,
        inside_border_insets: Insets {
            top: 3,
            left: 6,
            bottom: 0,
            right: 6,
        },
        minimum_cross_axis_size: TABBAR_H - 3,
        cross_axis_alignment: AxisAlignment::STRETCH,
        ..Default::default()
    }));
    let status = panel_create(None).expect("panel");
    status.set_to_box_layout(Some(&BoxLayoutSettings {
        horizontal: 1,
        between_child_spacing: 12,
        inside_border_insets: Insets {
            top: 0,
            left: 0,
            bottom: 0,
            right: 8,
        },
        minimum_cross_axis_size: STATUS_H,
        cross_axis_alignment: AxisAlignment::STRETCH,
        ..Default::default()
    }));
    FRAME.with_borrow_mut(|f| *f = Some((tabbar.clone(), status.clone())));
    (tabbar, status)
}

pub fn build() {
    let Some((tabbar, panel)) = FRAME.with_borrow(|f| f.clone()) else {
        return;
    };
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

    BAR.with_borrow_mut(|b| {
        *b = Some(Bar {
            mode,
            url,
            right,
            url_text: String::new(),
            tabbar: tabbar.clone(),
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
    with(|b| View::from(&b.mode).set_background_color(b.mode_color));
    tabs::refresh_tabbar();
}

pub fn set_mode(name: &str, color: u32) {
    with(|b| {
        // lualine-style pill: mode colour background, dark text.
        b.mode.set_text(Some(&format!(" {name} ").as_str().into()));
        b.mode_color = color;
        View::from(&b.mode).set_background_color(color);
    });
}

pub fn set_url(url: &str) {
    with(|b| {
        b.url_text = url.to_string();
        let color = if url.starts_with("https://") {
            GREEN
        } else if url.starts_with("http://") {
            RED
        } else {
            TEXT
        };
        set_color(&b.url, color);
        b.url.set_text(Some(&url.into()));
    });
}

/// One-shot message in place of the URL; the next key press restores the URL.
pub fn message(text: &str, error: bool) {
    with(|b| {
        set_color(&b.url, if error { RED } else { YELLOW });
        b.url.set_text(Some(&text.into()));
    });
}

pub fn clear_message() {
    let url = with(|b| b.url_text.clone()).unwrap_or_default();
    set_url(&url);
}

pub fn set_right(text: &str) {
    with(|b| b.right.set_text(Some(&text.into())));
}

pub fn set_bar_visible(on: bool) {
    FRAME.with_borrow(|f| f.as_ref().map(|f| View::from(&f.1).set_visible(on as i32)));
}

pub fn set_tabbar_visible(on: bool) {
    FRAME.with_borrow(|f| f.as_ref().map(|f| View::from(&f.0).set_visible(on as i32)));
    tabs::refresh_tabbar();
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
    let Some(bar) = BAR.with_borrow(|b| b.as_ref().map(|b| b.tabbar.clone())) else {
        return;
    };
    if View::from(&bar).is_visible() == 0 {
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
