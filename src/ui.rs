//! Native status bar + command line (CEF Views widgets): no extra renderer process,
//! no page CSS to fight. Catppuccin Mocha, like the vimb config.
//!
//! [ MODE ][ :command line… ][ url / message                    ][ [2/5] 42% ]

use crate::{keys, tabs, vim};
use cef::*;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

pub const FONT: &str = "CaskaydiaMono Nerd Font Mono, 15px";
const FONT_BOLD: &str = "CaskaydiaMono Nerd Font Mono, Bold 15px";
const BAR_BG: u32 = 0xFF18_1825;
const INPUT_BG: u32 = 0xFF1E_1E2E;
const TEXT: u32 = 0xFFCD_D6F4;
const SUBTEXT: u32 = 0xFFA6_ADC8;
const GREEN: u32 = 0xFFA6_E3A1;
const RED: u32 = 0xFFF3_8BA8;

/// Mode colours.
pub const BLUE: u32 = 0xFF89_B4FA;
pub const YELLOW: u32 = 0xFFF9_E2AF;
pub const MAUVE: u32 = 0xFFCB_A6F7;
pub const PEACH: u32 = 0xFFFA_B387;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmdKind {
    Ex,
    Search { forward: bool },
}

struct Bar {
    panel: Panel,
    mode: LabelButton,
    input: Textfield,
    url: LabelButton,
    right: LabelButton,
    kind: Option<CmdKind>,
    /// Waiting for the register name after <C-r>.
    reg_pending: bool,
    history: Vec<String>,
    hist_pos: usize,
    url_text: String,
}

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Bar) -> R) -> Option<R> {
    BAR.with_borrow_mut(|b| b.as_mut().map(f))
}

fn set_color(l: &LabelButton, color: u32) {
    for state in [ButtonState::NORMAL, ButtonState::HOVERED, ButtonState::PRESSED] {
        l.set_text_color(state, color);
    }
}

fn label(text: &str, color: u32) -> LabelButton {
    let mut d = NoopButton::new();
    let l = label_button_create(Some(&mut d), Some(&text.into())).expect("label");
    l.set_font_list(Some(&FONT.into()));
    set_color(&l, color);
    View::from(&l).set_focusable(0);
    l
}

/// Builds the bar; the caller adds it under the tab area.
pub fn build() -> Panel {
    let panel = panel_create(None).expect("panel");
    let layout = panel.set_to_box_layout(Some(&BoxLayoutSettings {
        horizontal: 1,
        between_child_spacing: 8,
        inside_border_insets: Insets { top: 2, left: 0, bottom: 2, right: 8 },
        cross_axis_alignment: AxisAlignment::CENTER,
        ..Default::default()
    }));
    View::from(&panel).set_background_color(BAR_BG);

    // Coloured text, not a pill: LabelButton paints an opaque themed background
    // over anything behind it, so a coloured parent panel never shows.
    let mode = label("NORMAL", BLUE);
    mode.set_font_list(Some(&FONT_BOLD.into()));
    let mut d = CmdDelegate::new();
    let input = textfield_create(Some(&mut d)).expect("textfield");
    input.set_font_list(Some(&FONT.into()));
    View::from(&input).set_background_color(INPUT_BG);
    View::from(&input).set_visible(0);
    let url = label("", TEXT);
    url.set_horizontal_alignment(HorizontalAlignment::LEFT);
    let right = label("", SUBTEXT);

    for v in [View::from(&mode), View::from(&input), View::from(&url), View::from(&right)] {
        panel.add_child_view(Some(&mut v.clone()));
    }
    if let Some(l) = layout {
        l.set_flex_for_view(Some(&mut View::from(&url)), 1);
        l.set_flex_for_view(Some(&mut View::from(&input)), 1);
    }
    BAR.with_borrow_mut(|b| {
        *b = Some(Bar {
            panel: panel.clone(),
            mode,
            input,
            url,
            right,
            kind: None,
            reg_pending: false,
            history: Vec::new(),
            hist_pos: 0,
            url_text: String::new(),
        })
    });
    panel
}

pub fn is_built() -> bool {
    BAR.with_borrow(|b| b.is_some())
}

pub fn set_mode(name: &str, color: u32) {
    with(|b| {
        b.mode.set_text(Some(&name.into()));
        set_color(&b.mode, color);
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
    with(|b| View::from(&b.panel).set_visible(on as i32));
}

/// Opens on the next UI tick: the key that triggered it (`:`, `o`) is still being
/// dispatched, and Views would hand that same press to the newly focused textfield.
pub fn cmdline_open(kind: CmdKind, text: &str) {
    let text = text.to_string();
    run_on_ui(0, move || cmdline_open_now(kind, &text));
}

fn cmdline_open_now(kind: CmdKind, text: &str) {
    with(|b| {
        b.kind = Some(kind);
        b.reg_pending = false;
        b.hist_pos = b.history.len();
        let prompt = match kind {
            CmdKind::Ex => ":",
            CmdKind::Search { forward: true } => "/",
            CmdKind::Search { forward: false } => "?",
        };
        b.mode.set_text(Some(&prompt.into()));
        set_color(&b.mode, PEACH);
        View::from(&b.url).set_visible(0);
        View::from(&b.input).set_visible(1);
        b.input.set_text(Some(&text.into()));
        recolor(&b.input);
        b.input.request_focus();
    });
}

fn cmdline_close() -> Option<(CmdKind, String)> {
    let r = with(|b| {
        let kind = b.kind.take()?;
        let text = CefString::from(&b.input.text()).to_string();
        View::from(&b.input).set_visible(0);
        View::from(&b.url).set_visible(1);
        if kind == CmdKind::Ex && !text.trim().is_empty() && b.history.last() != Some(&text) {
            b.history.push(text.clone());
        }
        Some((kind, text))
    })
    .flatten();
    tabs::focus_active();
    r
}

/// Theme colours for textfields were removed from CEF 152's API: paint the text instead.
fn recolor(t: &Textfield) {
    let len = CefString::from(&t.text()).to_string().encode_utf16().count() as u32;
    t.apply_text_color(TEXT, Some(&Range { from: 0, to: len }));
}

fn insert(t: &Textfield, s: &str) {
    t.insert_or_replace_text(Some(&s.into()));
    recolor(t);
}

/// Command-line keys. Returns true when handled (the textfield must not see it).
fn cmdline_key(t: &Textfield, k: &str) -> bool {
    if with(|b| std::mem::take(&mut b.reg_pending)).unwrap_or(false) {
        if let Some(text) = vim::register(k) {
            insert(t, &text);
        }
        return true;
    }
    match k {
        "<CR>" => {
            if let Some((kind, text)) = cmdline_close() {
                vim::cmdline_submit(kind, &text);
            }
        }
        "<Esc>" | "<C-c>" | "<C-[>" => {
            if let Some((kind, _)) = cmdline_close() {
                vim::cmdline_cancel(kind);
            }
        }
        "<C-r>" => {
            with(|b| b.reg_pending = true);
        }
        "<C-u>" => {
            t.set_text(Some(&"".into()));
        }
        "<C-w>" => {
            let text = CefString::from(&t.text()).to_string();
            let cur: Vec<char> = text.chars().collect();
            let pos = (t.cursor_position() as usize).min(cur.len());
            let mut start = pos;
            while start > 0 && cur[start - 1] == ' ' {
                start -= 1;
            }
            while start > 0 && cur[start - 1] != ' ' {
                start -= 1;
            }
            t.select_range(Some(&Range { from: start as u32, to: pos as u32 }));
            insert(t, "");
        }
        "<C-b>" | "<Home>" => t.select_range(Some(&Range { from: 0, to: 0 })),
        "<C-e>" | "<End>" => {
            let n = CefString::from(&t.text()).to_string().encode_utf16().count() as u32;
            t.select_range(Some(&Range { from: n, to: n }));
        }
        "<Up>" | "<Down>" => {
            let entry = with(|b| {
                let n = b.history.len();
                b.hist_pos = if k == "<Up>" { b.hist_pos.saturating_sub(1) } else { (b.hist_pos + 1).min(n) };
                b.history.get(b.hist_pos).cloned().unwrap_or_default()
            });
            t.set_text(Some(&entry.unwrap_or_default().as_str().into()));
            recolor(t);
        }
        // Completion arrives with the polished UI (phase 3); keep focus here meanwhile.
        "<Tab>" | "<S-Tab>" => {}
        _ => return false,
    }
    true
}

wrap_textfield_delegate! {
    struct CmdDelegate {}

    impl ViewDelegate {}

    impl TextfieldDelegate {
        fn on_key_event(&self, textfield: Option<&mut Textfield>, event: Option<&KeyEvent>) -> i32 {
            let (Some(t), Some(e)) = (textfield, event) else { return 0 };
            if e.type_ != KeyEventType::RAWKEYDOWN {
                return 0;
            }
            keys::from_event(e).is_some_and(|k| cmdline_key(t, &k)) as i32
        }

        fn on_after_user_action(&self, textfield: Option<&mut Textfield>) {
            let Some(t) = textfield else { return };
            recolor(t);
            if let Some(Some(CmdKind::Search { forward })) = with(|b| b.kind) {
                vim::search_preview(&CefString::from(&t.text()).to_string(), forward);
            }
        }
    }
}

wrap_button_delegate! {
    struct NoopButton {}

    impl ViewDelegate {}

    impl ButtonDelegate {
        fn on_button_pressed(&self, _button: Option<&mut Button>) {}
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
