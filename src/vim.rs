//! The vim engine: modes, counts, pending keys, built-in keys, user maps, hints.
//!
//! Keys arrive from `KeyboardHandler::on_pre_key_event` *before* the page sees them;
//! returning true swallows the key. Insert mode is Chromium's own focus state
//! (`focus_on_editable_field`), so typing in any field just works.

use crate::commands::{self, opt, opt_num};
use crate::editor;
use crate::keys::{self, Key};
use crate::popup::{self, CmdKind};
use crate::tabs;
use crate::ui;
use cef::{ImplBrowser, KeyEvent};
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Insert,
    Hint,
    Pass,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HintAct {
    Click,
    BgTab,
    Yank,
    YankText,
    XCommand,
    PromptOpen,
    PromptTab,
    /// ;i / ;I (new tab)
    OpenImage(bool),
    /// ;s
    Save,
    /// ;k
    Remove,
    /// ;p (queue end) / ;P (front)
    Queue(bool),
    /// ;e
    Edit,
    /// ;m
    Mpv,
}

#[derive(Clone, Copy, Debug)]
enum Act {
    Scroll(i32, i32),
    Pages(f32),
    Top,
    Bottom,
    Left,
    Right,
    /// Action + what may be hinted (page.js FILTERS: all, links, images, editable).
    Hint(HintAct, &'static str),
    Prompt(&'static str),
    PromptUrl(&'static str),
    Search(bool),
    SearchNext(bool),
    YankUrl,
    Paste(bool),
    Reload,
    ReloadNoCache,
    Stop,
    Back,
    Forward,
    Reopen(bool),
    TabNext,
    TabPrev,
    TabFirst,
    TabLast,
    NewTab,
    CloseTab,
    UrlUp,
    UrlRoot,
    Home(bool),
    FocusInput,
    Pass,
    Zoom(f64),
    ZoomReset,
    QuitAll,
    Escape,
    ViewSource,
    Inspector,
    Pip,
}

/// vimb's default normal-mode keys (`man vimb`), plus Ctrl-T/W/Tab for muscle memory.
/// (keys, action, description for the :keys cheatsheet)
const BUILTINS: &[(&str, Act, &str)] = &[
    ("j", Act::Scroll(0, 1), "scroll down"),
    ("k", Act::Scroll(0, -1), "scroll up"),
    ("h", Act::Scroll(-1, 0), "scroll left"),
    ("l", Act::Scroll(1, 0), "scroll right"),
    ("<C-d>", Act::Pages(0.5), "half page down"),
    ("<C-u>", Act::Pages(-0.5), "half page up"),
    ("<C-f>", Act::Pages(1.0), "page down"),
    ("<C-b>", Act::Pages(-1.0), "page up"),
    ("<Space>", Act::Pages(0.9), "page down"),
    ("gg", Act::Top, "top of page"),
    ("G", Act::Bottom, "bottom of page"),
    ("0", Act::Left, "scroll far left"),
    ("^", Act::Left, "scroll far left"),
    ("$", Act::Right, "scroll far right"),
    ("f", Act::Hint(HintAct::Click, "all"), "hint: open link"),
    (
        "F",
        Act::Hint(HintAct::BgTab, "links"),
        "hint: open in new tab",
    ),
    (";o", Act::Hint(HintAct::Click, "all"), "hint: open link"),
    (
        ";t",
        Act::Hint(HintAct::BgTab, "links"),
        "hint: open in new tab",
    ),
    (";y", Act::Hint(HintAct::Yank, "links"), "hint: yank link"),
    (";Y", Act::Hint(HintAct::YankText, "all"), "hint: yank text"),
    (
        ";x",
        Act::Hint(HintAct::XCommand, "links"),
        "hint: run x-hint-command",
    ),
    (
        ";O",
        Act::Hint(HintAct::PromptOpen, "links"),
        "hint: :open with link",
    ),
    (
        ";T",
        Act::Hint(HintAct::PromptTab, "links"),
        "hint: :tabopen with link",
    ),
    (
        ";i",
        Act::Hint(HintAct::OpenImage(false), "images"),
        "hint: open image",
    ),
    (
        ";I",
        Act::Hint(HintAct::OpenImage(true), "images"),
        "hint: open image in new tab",
    ),
    (";s", Act::Hint(HintAct::Save, "links"), "hint: save link"),
    (
        ";k",
        Act::Hint(HintAct::Remove, "all"),
        "hint: remove element",
    ),
    (
        ";p",
        Act::Hint(HintAct::Queue(false), "links"),
        "hint: read later",
    ),
    (
        ";P",
        Act::Hint(HintAct::Queue(true), "links"),
        "hint: read later (front)",
    ),
    (
        ";e",
        Act::Hint(HintAct::Edit, "editable"),
        "hint: edit field in editor",
    ),
    (
        ";m",
        Act::Hint(HintAct::Mpv, "media"),
        "hint: play in mpv",
    ),
    ("gf", Act::ViewSource, "view page source"),
    ("gF", Act::Inspector, "DevTools"),
    ("gp", Act::Pip, "picture-in-picture (main video)"),
    ("o", Act::Prompt("open "), ":open"),
    ("O", Act::PromptUrl("open "), ":open with current URL"),
    ("t", Act::Prompt("tabopen "), ":tabopen"),
    ("T", Act::PromptUrl("tabopen "), ":tabopen with current URL"),
    (":", Act::Prompt(""), "command line"),
    ("/", Act::Search(true), "search forward"),
    ("?", Act::Search(false), "search backward"),
    ("n", Act::SearchNext(false), "next match"),
    ("N", Act::SearchNext(true), "previous match"),
    ("y", Act::YankUrl, "yank page URL"),
    ("p", Act::Paste(false), "open clipboard URL"),
    ("P", Act::Paste(true), "open clipboard URL in new tab"),
    ("r", Act::Reload, "reload"),
    ("R", Act::ReloadNoCache, "reload, no cache"),
    ("<C-c>", Act::Stop, "stop loading"),
    ("<C-o>", Act::Back, "back"),
    ("<C-i>", Act::Forward, "forward"),
    ("u", Act::Reopen(false), "reopen closed tab"),
    ("U", Act::Reopen(true), "reopen closed in new tab"),
    ("gt", Act::TabNext, "next tab"),
    ("gT", Act::TabPrev, "previous tab"),
    ("g0", Act::TabFirst, "first tab"),
    ("g$", Act::TabLast, "last tab"),
    ("<C-Tab>", Act::TabNext, "next tab"),
    ("<C-S-Tab>", Act::TabPrev, "previous tab"),
    ("<C-t>", Act::NewTab, "new tab"),
    ("<C-w>", Act::CloseTab, "close tab"),
    ("gu", Act::UrlUp, "go up one URL level"),
    ("gU", Act::UrlRoot, "go to domain root"),
    ("gh", Act::Home(false), "home page"),
    ("gH", Act::Home(true), "home page in new tab"),
    ("gi", Act::FocusInput, "focus first input"),
    ("i", Act::FocusInput, "focus input"),
    ("<C-z>", Act::Pass, "pass-through mode"),
    ("zi", Act::Zoom(0.5), "zoom in"),
    ("zo", Act::Zoom(-0.5), "zoom out"),
    ("zz", Act::ZoomReset, "reset zoom"),
    ("zI", Act::Zoom(0.5), "zoom in"),
    ("zO", Act::Zoom(-0.5), "zoom out"),
    ("<C-=>", Act::Zoom(0.5), "zoom in"),
    ("<C-+>", Act::Zoom(0.5), "zoom in"),
    ("<C-->", Act::Zoom(-0.5), "zoom out"),
    ("<C-0>", Act::ZoomReset, "reset zoom"),
    ("<C-q>", Act::QuitAll, "quit"),
    ("<Esc>", Act::Escape, "normal mode"),
];

struct Map {
    rhs: String,
    noremap: bool,
    desc: Option<String>,
}

struct Vim {
    mode: Mode,
    pending: Vec<Key>,
    count: Option<u32>,
    /// Bumped on every key; a timeout only fires if no key came since.
    generation: u64,
    builtins: HashMap<Vec<Key>, Act>,
    maps: HashMap<char, HashMap<Vec<Key>, Map>>,
    hint_act: HintAct,
    last_hint: String,
    last_cmd: String,
    last_search: Option<(String, bool)>,
    /// Esc left a text field. Chat apps put the focus straight back into their
    /// message box; without this the next key would be typed there again. Normal
    /// mode holds until the user picks a field (i, gi, a click or hint, Tab) or the
    /// page changes.
    escaped: bool,
}

thread_local! {
    static VIM: RefCell<Vim> = RefCell::new(Vim {
        mode: Mode::Normal,
        pending: Vec::new(),
        count: None,
        generation: 0,
        builtins: BUILTINS.iter().map(|(k, a, _)| (keys::parse(k), *a)).collect(),
        maps: HashMap::new(),
        hint_act: HintAct::Click,
        last_hint: String::new(),
        last_cmd: String::new(),
        last_search: None,
        escaped: false,
    });
}

/// vim's timeoutlen: how long an ambiguous prefix (exact map + longer map) waits.
const TIMEOUT_MS: i64 = 1000;

pub fn map(mode: &str, lhs: &str, rhs: &str, noremap: bool, desc: Option<String>) {
    let m = mode.chars().next().unwrap_or('n');
    VIM.with_borrow_mut(|v| {
        v.maps.entry(m).or_default().insert(
            keys::parse(lhs),
            Map {
                rhs: rhs.into(),
                noremap,
                desc,
            },
        );
    });
}

/// (keys, description) for the :keys cheatsheet: normal-mode maps, then the
/// built-ins they don't shadow.
pub fn cheatsheet() -> Vec<(String, String)> {
    VIM.with_borrow(|v| {
        let maps = v.maps.get(&'n');
        let mut out: Vec<(String, String)> = maps
            .into_iter()
            .flatten()
            .map(|(k, m)| (k.concat(), m.desc.clone().unwrap_or_else(|| m.rhs.clone())))
            .collect();
        out.sort();
        for (k, _, d) in BUILTINS {
            if !maps.is_some_and(|m| m.contains_key(&keys::parse(k))) {
                out.push((k.to_string(), d.to_string()));
            }
        }
        out
    })
}

pub fn unmap(mode: &str, lhs: &str) {
    let m = mode.chars().next().unwrap_or('n');
    VIM.with_borrow_mut(|v| v.maps.entry(m).or_default().remove(&keys::parse(lhs)));
}

fn set_mode(mode: Mode) {
    VIM.with_borrow_mut(|v| v.mode = mode);
    show_mode();
}

fn show_mode() {
    let (mode, pending, count) = VIM.with_borrow(|v| (v.mode, v.pending.concat(), v.count));
    let extra = format!(
        "{}{pending}",
        count.map(|c| c.to_string()).unwrap_or_default()
    );
    let (name, color) = match mode {
        Mode::Normal if !extra.is_empty() => (extra.as_str(), ui::BLUE),
        Mode::Normal => ("NORMAL", ui::BLUE),
        Mode::Insert => ("INSERT", 0xFFA6_E3A1),
        Mode::Hint => ("HINT", ui::YELLOW),
        Mode::Pass => ("PASS", ui::MAUVE),
    };
    ui::set_mode(name, color);
}

/// Another page has the keys now (tab, window, navigation): a field focused there
/// means insert mode again.
pub fn new_page() {
    VIM.with_borrow_mut(|v| v.escaped = false);
    reset();
}

/// Active tab changed or page reported focus: leave Insert/Hint, keep Pass.
pub fn reset() {
    VIM.with_borrow_mut(|v| {
        v.pending.clear();
        v.count = None;
        if v.mode != Mode::Pass {
            v.mode = Mode::Normal;
        }
    });
    show_mode();
}

/// A RAWKEYDOWN in a tab. Returns true to swallow it.
pub fn on_key(e: &KeyEvent) -> bool {
    let Some(k) = keys::from_event(e) else {
        return false;
    };
    if std::env::var_os("REMBROWSER_DEBUG_KEYS").is_some() {
        let mode = VIM.with_borrow(|v| v.mode);
        eprintln!(
            "key {k:?} mode={mode:?} editable={}",
            e.focus_on_editable_field
        );
    }
    ui::clear_message();
    let (mode, escaped) = VIM.with_borrow_mut(|v| {
        v.generation += 1;
        (v.mode, v.escaped)
    });
    // CEF's flag misses autofocused fields (no focus change after load); page.js's
    // focus report (mode Insert) catches those. After Esc the flag doesn't count:
    // the page may have taken the focus back.
    let editable = mode == Mode::Insert || (e.focus_on_editable_field != 0 && !escaped);
    match mode {
        Mode::Pass => {
            if k == "<Esc>" {
                set_mode(Mode::Normal);
                return true;
            }
            false
        }
        Mode::Hint => {
            hint_key(&k);
            true
        }
        _ if editable => {
            if mode != Mode::Insert {
                set_mode(Mode::Insert);
            }
            insert_key(&k)
        }
        _ => {
            if mode == Mode::Insert {
                set_mode(Mode::Normal);
            }
            normal_key(k)
        }
    }
}

/// Browser keys that mean nothing in a text field, so they work in insert mode too.
/// (<C-w> stays the field's delete-word, like vim/readline.)
/// (<C-t> is vimb's input-mode "edit in editor", handled below.)
const GLOBAL_KEYS: &[&str] = &[
    "<C-Tab>",
    "<C-S-Tab>",
    "<C-q>",
    "<C-=>",
    "<C-+>",
    "<C-->",
    "<C-0>",
];

fn insert_key(k: &str) -> bool {
    if GLOBAL_KEYS.contains(&k) {
        let a = VIM.with_borrow(|v| v.builtins.get(&vec![k.to_string()]).copied());
        if let Some(a) = a {
            set_mode(Mode::Normal);
            act(a, 1);
            return true;
        }
    }
    match k {
        "<Esc>" | "<C-[>" => {
            tabs::js("__rem.blur()");
            VIM.with_borrow_mut(|v| v.escaped = true);
            set_mode(Mode::Normal);
            true
        }
        "<C-z>" => {
            set_mode(Mode::Pass);
            true
        }
        // vimb: edit the field in editor-command (nvim popup).
        "<C-t>" => {
            EDIT_WANTED.set(Some(std::time::Instant::now()));
            tabs::js("__rem.editGet()");
            true
        }
        _ => {
            let rhs = VIM.with_borrow(|v| {
                v.maps
                    .get(&'i')
                    .and_then(|m| m.get(&vec![k.to_string()]))
                    .map(|m| (m.rhs.clone(), m.noremap))
            });
            match rhs {
                Some((rhs, noremap)) => {
                    run_keys(&rhs, noremap);
                    true
                }
                None => false,
            }
        }
    }
}

enum Lookup {
    User(String, bool),
    Builtin(Act),
}

fn lookup(v: &Vim, seq: &[Key], user: bool) -> (Option<Lookup>, bool) {
    let maps = v.maps.get(&'n').filter(|_| user);
    let exact = maps
        .and_then(|m| m.get(seq))
        .map(|m| Lookup::User(m.rhs.clone(), m.noremap))
        .or_else(|| v.builtins.get(seq).map(|a| Lookup::Builtin(*a)));
    let longer = |k: &Vec<Key>| k.len() > seq.len() && k.starts_with(seq);
    let prefix = maps.is_some_and(|m| m.keys().any(longer)) || v.builtins.keys().any(longer);
    (exact, prefix)
}

fn normal_key(k: Key) -> bool {
    enum Next {
        Wait,
        WaitThenRun(u64),
        Run(Lookup, u32),
        Pass,
        Swallow,
    }
    let next = VIM.with_borrow_mut(|v| {
        // Counts: 3j, 10G. A leading 0 is a key (scroll left), not a count.
        if v.pending.is_empty()
            && k.len() == 1
            && k.chars().all(|c| c.is_ascii_digit())
            && (k != "0" || v.count.is_some())
        {
            let d = k.parse::<u32>().unwrap_or(0);
            v.count = Some(
                v.count
                    .unwrap_or(0)
                    .saturating_mul(10)
                    .saturating_add(d)
                    .min(9999),
            );
            return Next::Wait;
        }
        v.pending.push(k);
        let (exact, prefix) = lookup(v, &v.pending, true);
        match (exact, prefix) {
            (Some(_), true) => Next::WaitThenRun(v.generation),
            (None, true) => Next::Wait,
            (Some(l), false) => {
                v.pending.clear();
                Next::Run(l, v.count.take().unwrap_or(1))
            }
            (None, false) => {
                // Unmapped single keys go to the page (site shortcuts keep working).
                // That includes Tab into a field: typing there is wanted again.
                let single = v.pending.len() == 1 && v.count.is_none();
                v.pending.clear();
                v.count = None;
                v.escaped &= !single;
                if single { Next::Pass } else { Next::Swallow }
            }
        }
    });
    show_mode();
    match next {
        Next::Wait | Next::Swallow => true,
        Next::Pass => false,
        Next::Run(l, count) => {
            run(l, count);
            true
        }
        Next::WaitThenRun(generation) => {
            ui::run_on_ui(TIMEOUT_MS, move || timeout(generation));
            true
        }
    }
}

fn timeout(generation: u64) {
    let job = VIM.with_borrow_mut(|v| {
        if v.generation != generation || v.pending.is_empty() {
            return None;
        }
        let (exact, _) = lookup(v, &v.pending, true);
        v.pending.clear();
        exact.map(|l| (l, v.count.take().unwrap_or(1)))
    });
    show_mode();
    if let Some((l, count)) = job {
        run(l, count);
    }
}

fn run(l: Lookup, count: u32) {
    match l {
        Lookup::User(rhs, noremap) => {
            for _ in 0..count.min(100) {
                run_keys(&rhs, noremap);
            }
        }
        Lookup::Builtin(a) => act(a, count),
    }
}

/// Replays a key sequence (map RHS, `:normal`). `:…<CR>` runs an ex command directly,
/// `:…` without <CR> opens the command line pre-filled (`:bma<Space>`).
pub fn run_keys(rhs: &str, noremap: bool) {
    let ks = keys::parse(rhs);
    let mut i = 0;
    let mut seq: Vec<Key> = Vec::new();
    while i < ks.len() {
        let k = &ks[i];
        i += 1;
        if seq.is_empty() && (k == ":" || k == "/" || k == "?") {
            let mut text = String::new();
            let mut done = false;
            while i < ks.len() {
                let k = &ks[i];
                i += 1;
                match k.as_str() {
                    "<CR>" => {
                        done = true;
                        break;
                    }
                    "<C-r>" if i < ks.len() => {
                        text.push_str(&register(&ks[i]).unwrap_or_default());
                        i += 1;
                    }
                    _ => text.push_str(keys::text(k).unwrap_or("")),
                }
            }
            let kind = match k.as_str() {
                ":" => CmdKind::Ex,
                f => CmdKind::Search { forward: f == "/" },
            };
            if done {
                cmdline_submit(kind, &text);
            } else {
                popup::open(kind, &text);
            }
            continue;
        }
        seq.push(k.clone());
        let (exact, prefix) = VIM.with_borrow(|v| lookup(v, &seq, !noremap));
        if prefix && i < ks.len() {
            continue;
        }
        if let Some(l) = exact {
            match l {
                // nmap (recursive) maps one level deep only: no infinite loops.
                Lookup::User(rhs, _) => run_keys(&rhs, true),
                Lookup::Builtin(a) => act(a, 1),
            }
        }
        seq.clear();
    }
}

fn act(a: Act, count: u32) {
    let n = count as i32;
    let step = opt_num("scroll-step", 60);
    let smooth = commands::opt_bool("smooth-scrolling", true);
    let scroll = |kind: &str, dx: i32, dy: i32| {
        tabs::js(&format!(
            "__rem.smooth={smooth};__rem.scroll('{kind}',{dx},{dy})"
        ));
    };
    match a {
        // Relative scrolling is a wheel tick (no page JS on the key path).
        Act::Scroll(dx, dy) if smooth => tabs::wheel(dx * step * n, dy * step * n),
        Act::Pages(p) if smooth => {
            tabs::wheel(0, (p * n as f32 * tabs::page_height() as f32) as i32)
        }
        Act::Scroll(dx, dy) => scroll("by", dx * step * n, dy * step * n),
        Act::Pages(p) => tabs::js(&format!(
            "__rem.smooth=false;__rem.scroll('page',0,{})",
            p * n as f32
        )),
        // 50G = 50% (vimb), plain G = bottom.
        Act::Top if count > 1 => scroll("percent", 0, n.min(100)),
        Act::Bottom if count > 1 => scroll("percent", 0, n.min(100)),
        Act::Top => scroll("top", 0, 0),
        Act::Bottom => scroll("bottom", 0, 0),
        Act::Left => scroll("left", 0, 0),
        Act::Right => scroll("right", 0, 0),
        Act::Hint(h, filter) => start_hints(h, filter),
        Act::ViewSource => tabs::open(&format!("view-source:{}", tabs::active_url()), false),
        Act::Inspector => ui::run_on_ui(0, tabs::devtools),
        Act::Pip => tabs::pip(),
        Act::Prompt(p) => popup::open(CmdKind::Ex, p),
        Act::PromptUrl(p) => popup::open(CmdKind::Ex, &format!("{p}{}", tabs::active_url())),
        Act::Search(forward) => popup::open(CmdKind::Search { forward }, ""),
        Act::SearchNext(reverse) => {
            if let Some((text, forward)) = VIM.with_borrow(|v| v.last_search.clone()) {
                tabs::find(&text, forward != reverse, true);
            }
        }
        Act::YankUrl => yank(&tabs::active_url()),
        Act::Paste(new_tab) => {
            let clip = commands::command("wl-paste")
                .arg("-n")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
            match clip {
                Ok(s) if !s.is_empty() => commands::open(&s, new_tab),
                Ok(_) => ui::message("clipboard is empty", true),
                // No wl-clipboard: ask the page's clipboard API (answer: "paste" message).
                Err(_) => {
                    PASTE_WANTED.set(Some((std::time::Instant::now(), new_tab)));
                    tabs::js_gesture(
                        "navigator.clipboard.readText().then((t) => console.debug('\\x01rem\\tpaste\\t' + t.replace(/[\\t\\n]/g, ' ')), \
                         () => console.debug('\\x01rem\\tpaste\\t'))",
                    );
                }
            }
        }
        Act::Reload => tabs::with_active(|b| b.reload()),
        Act::ReloadNoCache => tabs::with_active(|b| b.reload_ignore_cache()),
        Act::Stop => tabs::with_active(|b| b.stop_load()),
        Act::Back => (0..n).for_each(|_| tabs::with_active(|b| b.go_back())),
        Act::Forward => (0..n).for_each(|_| tabs::with_active(|b| b.go_forward())),
        Act::Reopen(new_tab) => tabs::reopen(new_tab),
        Act::TabNext if count > 1 => tabs::select(count as usize - 1), // 3gt = tab 3 (vim)
        Act::TabNext => tabs::cycle(1),
        Act::TabPrev => tabs::cycle(-n as isize),
        Act::TabFirst => tabs::select(0),
        Act::TabLast => tabs::select(usize::MAX),
        Act::NewTab => tabs::open(&commands::resolve_url(""), false),
        Act::CloseTab => tabs::close_active(),
        Act::UrlUp => tabs::load(&url_up(&tabs::active_url())),
        Act::UrlRoot => tabs::load(&url_root(&tabs::active_url())),
        Act::Home(new_tab) => commands::open("", new_tab),
        Act::FocusInput => {
            VIM.with_borrow_mut(|v| v.escaped = false);
            tabs::js("__rem.focusInput()")
        }
        Act::Pass => set_mode(Mode::Pass),
        Act::Zoom(d) => zoom(d * n as f64),
        Act::ZoomReset => zoom(0.0),
        Act::QuitAll => tabs::quit_all(),
        Act::Escape => {
            tabs::stop_find();
            tabs::js("__rem.hintsClear()");
        }
    }
}

/// Chrome's zoom steps (%).
const ZOOM_STEPS: &[f64] = &[
    25.0, 33.0, 50.0, 67.0, 75.0, 80.0, 90.0, 100.0, 110.0, 125.0, 150.0, 175.0, 200.0, 250.0,
    300.0, 400.0, 500.0,
];

/// Next step up (`delta` > 0) or down from `pct`; 0 = back to 100%.
fn zoom_step(pct: f64, delta: f64) -> f64 {
    if delta > 0.0 {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|&s| s > pct + 0.5)
            .unwrap_or(500.0)
    } else if delta < 0.0 {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|&s| s < pct - 0.5)
            .unwrap_or(25.0)
    } else {
        100.0
    }
}

/// Chromium zoom level ↔ percent: 100% · 1.2^level.
pub fn zoom(delta: f64) {
    let pct = 1.2f64.powf(tabs::zoom()) * 100.0;
    let next = zoom_step(pct, delta);
    tabs::set_zoom((next / 100.0).ln() / 1.2f64.ln());
    ui::message(&format!("zoom {next:.0}%"), false);
}

fn yank(text: &str) {
    if commands::command("wl-copy")
        .arg("--")
        .arg(text)
        .spawn()
        .is_err()
    {
        // No wl-clipboard: copy from inside the page, as a user gesture (no permission
        // needed, works on http pages too), then give focus back.
        tabs::js_gesture(&format!(
            "((t) => {{ const a = document.activeElement, d = document.createElement('textarea'); d.value = t; \
             d.style.cssText = 'position:fixed;opacity:0'; document.documentElement.append(d); d.select(); \
             document.execCommand('copy'); d.remove(); a?.focus?.(); }})({})",
            popup::js_str(text)
        ));
    }
    ui::message(&format!("yanked {text}"), false);
}

thread_local! {
    /// A `p`/`P` waiting for the page's clipboard answer (only without wl-paste).
    static PASTE_WANTED: std::cell::Cell<Option<(std::time::Instant, bool)>> = const { std::cell::Cell::new(None) };
    /// Ctrl-T / ;e waiting for the page to hand over the field's text.
    static EDIT_WANTED: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// `https://a.com/b/c/?q` → `https://a.com/b/`.
fn url_up(url: &str) -> String {
    let root = url_root(url);
    let base = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    match base.rfind('/') {
        Some(i) if base.len() >= root.len() && i + 1 >= root.len() => format!("{}/", &base[..i]),
        _ => root,
    }
}

fn url_root(url: &str) -> String {
    match url.find("://") {
        Some(s) => {
            let host_end = url[s + 3..].find('/').map_or(url.len(), |i| s + 3 + i);
            format!("{}/", &url[..host_end])
        }
        None => url.into(),
    }
}

// ── command line ─────────────────────────────────────────────────

/// A URL as it goes into a map or the command line. Maps hand it to the shell
/// (`:sh! mpv '<C-R>%'`), and a page chooses its own URLs: a quote in one would end
/// the quoting and run the rest as a command. Those characters are percent-encoded
/// (same URL), which makes it safe inside single or double quotes.
/// Not safe unquoted (`&`, `;` and `|` are ordinary URL characters): quote it, or use
/// $REMBROWSER_URI.
fn shell_safe(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        match c {
            '\'' | '"' | '`' | '$' | '\\' | '!' => out.push_str(&format!("%{:02X}", c as u32)),
            c if c.is_whitespace() || c.is_control() => {
                out.push_str(&format!("%{:02X}", c as u32))
            }
            c => out.push(c),
        }
    }
    out
}

/// <C-r>{reg}: % URL, ; last hinted URL, : last command, / last search.
pub fn register(k: &str) -> Option<String> {
    match k {
        "%" => Some(shell_safe(&tabs::active_url())),
        ";" => VIM.with_borrow(|v| Some(shell_safe(&v.last_hint))),
        ":" => VIM.with_borrow(|v| Some(v.last_cmd.clone())),
        "/" => VIM.with_borrow(|v| v.last_search.as_ref().map(|s| s.0.clone())),
        _ => None,
    }
}

pub fn cmdline_submit(kind: CmdKind, text: &str) {
    match kind {
        CmdKind::Ex => {
            VIM.with_borrow_mut(|v| v.last_cmd = text.into());
            commands::exec(text);
        }
        CmdKind::Search { forward } => {
            if text.is_empty() {
                return;
            }
            VIM.with_borrow_mut(|v| v.last_search = Some((text.into(), forward)));
            tabs::find(text, forward, false);
        }
    }
}

pub fn cmdline_cancel(kind: CmdKind) {
    if matches!(kind, CmdKind::Search { .. }) {
        tabs::stop_find();
    }
}

pub fn search_preview(text: &str, forward: bool) {
    if commands::opt_bool("incsearch", true) {
        if text.is_empty() {
            tabs::stop_find();
        } else {
            tabs::find(text, forward, false);
        }
    }
}

// ── hints ────────────────────────────────────────────────────────

fn start_hints(h: HintAct, filter: &str) {
    VIM.with_borrow_mut(|v| v.hint_act = h);
    set_mode(Mode::Hint);
    let keys = opt("hint-keys").unwrap_or_else(|| "asdfghjkl".into());
    // JSON-quote the keys: they come from the config file.
    let keys: String = keys.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    tabs::js(&format!("__rem.hints('{keys}','{filter}')"));
}

fn hint_key(k: &str) {
    match k {
        "<Esc>" | "<C-c>" | "<C-[>" => {
            tabs::js("__rem.hintsClear()");
            set_mode(Mode::Normal);
        }
        "<BS>" => tabs::js("__rem.hintKey('<BS>')"),
        _ if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric()) => {
            tabs::js(&format!("__rem.hintKey('{}')", k.to_ascii_lowercase()));
        }
        _ => {}
    }
}

/// Messages from page.js in the active tab.
pub fn on_page_msg(fields: &[&str]) {
    match fields {
        // A real click (mouse or hint) into the page: a field it lands in is wanted.
        ["user"] => VIM.with_borrow_mut(|v| v.escaped = false),
        // Focus the page took back by itself after Esc doesn't count.
        ["focus", "1"] if VIM.with_borrow(|v| v.escaped) => {}
        ["focus", on] => {
            let mode = VIM.with_borrow(|v| v.mode);
            if matches!(mode, Mode::Normal | Mode::Insert) {
                set_mode(if *on == "1" {
                    Mode::Insert
                } else {
                    Mode::Normal
                });
            }
        }
        // Only the answer to our own paste request (a page can't open tabs this way).
        ["paste", text] => match PASTE_WANTED.take() {
            Some((at, new_tab)) if at.elapsed().as_secs() < 30 && !text.trim().is_empty() => {
                commands::open(text.trim(), new_tab)
            }
            Some(_) => ui::message("clipboard is empty", true),
            None => {}
        },
        ["pip-none"] => ui::message("no video on this page", true),
        ["edit-none"] => ui::message("no text field focused", true),
        // Only the answer to our own request (Ctrl-T, ;e): a page can't open the
        // editor by itself.
        ["edit-text", text] => {
            if EDIT_WANTED.take().is_some_and(|at| at.elapsed().as_secs() < 5) {
                edit_field(editor::percent_decode(text));
            }
        }
        ["hint-none"] if VIM.with_borrow(|v| v.mode) == Mode::Hint => {
            set_mode(Mode::Normal);
            ui::message("no hints", true);
        }
        // Only accepted while we are hinting: a page can't fake a pick at other times.
        ["hint-pick", href, text, x, y, _editable] if VIM.with_borrow(|v| v.mode) == Mode::Hint => {
            set_mode(Mode::Normal);
            let (x, y) = (x.parse().unwrap_or(0.0), y.parse().unwrap_or(0.0));
            hint_picked(href, text, x, y);
        }
        _ => {}
    }
}

fn hint_picked(href: &str, text: &str, x: f64, y: f64) {
    let act = VIM.with_borrow_mut(|v| {
        if !href.is_empty() {
            v.last_hint = href.into();
        }
        v.hint_act
    });
    match act {
        // A real mouse click: counts as a user gesture (target=_blank, JS handlers).
        HintAct::Click => tabs::click(x, y),
        HintAct::BgTab if !href.is_empty() => tabs::open(href, true),
        HintAct::BgTab => tabs::click(x, y),
        HintAct::Yank => yank(href),
        HintAct::YankText => yank(text),
        HintAct::XCommand => {
            if let Some(cmd) = opt("x-hint-command") {
                run_keys(&cmd, true);
            }
        }
        HintAct::PromptOpen => popup::open(CmdKind::Ex, &format!("open {href}")),
        HintAct::PromptTab => popup::open(CmdKind::Ex, &format!("tabopen {href}")),
        HintAct::OpenImage(new_tab) if new_tab => tabs::open(href, false),
        HintAct::OpenImage(_) => tabs::load(href),
        HintAct::Save => tabs::download(href),
        HintAct::Remove => tabs::js("__rem.picked?.remove()"),
        HintAct::Queue(front) => commands::queue_add(href, front),
        HintAct::Edit => {
            EDIT_WANTED.set(Some(std::time::Instant::now()));
            tabs::js("__rem.editGet(__rem.picked)")
        }
        HintAct::Mpv => commands::mpv(href),
    }
}

/// Field text → editor-command → back into the same field of the same tab.
fn edit_field(text: String) {
    let Some(id) = tabs::active_id() else { return };
    let had_newline = text.ends_with('\n');
    editor::edit(&text, &editor::field_command(), move |mut new| {
        // Editors end the file with a newline; don't add one to a one-line field.
        if !had_newline && new.ends_with('\n') {
            new.pop();
        }
        tabs::js_in(id, &format!("__rem.editSet({})", popup::js_str(&new)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps() {
        assert_eq!(zoom_step(100.0, 1.0), 110.0);
        assert_eq!(zoom_step(110.0, 1.0), 125.0);
        assert_eq!(zoom_step(100.0, -1.0), 90.0);
        assert_eq!(zoom_step(119.0, -1.0), 110.0);
        assert_eq!(zoom_step(500.0, 1.0), 500.0);
        assert_eq!(zoom_step(140.0, 0.0), 100.0);
    }

    #[test]
    fn urls_cannot_break_out_of_shell_quotes() {
        // What a hostile page would name itself to hijack `:sh! mpv '<C-R>%'`.
        let s = shell_safe("https://evil.example/a';touch ~/pwned;'?q=$(id)&x=`id`\"\\ b");
        assert!(!s.contains(['\'', '"', '`', '$', '\\', ' ']));
        assert_eq!(s, "https://evil.example/a%27;touch%20~/pwned;%27?q=%24(id)&x=%60id%60%22%5C%20b");
        // An ordinary URL is untouched.
        let ok = "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=10s#top";
        assert_eq!(shell_safe(ok), ok);
    }

    #[test]
    fn url_navigation() {
        assert_eq!(url_up("https://a.com/b/c/?q=1"), "https://a.com/b/");
        assert_eq!(url_up("https://a.com/b"), "https://a.com/");
        assert_eq!(url_up("https://a.com/"), "https://a.com/");
        assert_eq!(url_root("https://a.com/b/c"), "https://a.com/");
    }

    #[test]
    fn user_map_beats_builtin_and_prefixes_wait() {
        map("n", "d", ":q<CR>", true, None);
        map("n", "<Space>ma", ":bma<CR>", true, None);
        VIM.with_borrow(|v| {
            assert!(matches!(
                lookup(v, &keys::parse("d"), true),
                (Some(Lookup::User(..)), false)
            ));
            assert!(matches!(
                lookup(v, &keys::parse("<Space>m"), true),
                (None, true)
            ));
            // <Space> alone is a builtin (page down) but also a prefix: must wait.
            assert!(matches!(
                lookup(v, &keys::parse("<Space>"), true),
                (Some(Lookup::Builtin(_)), true)
            ));
            assert!(matches!(lookup(v, &keys::parse("g"), true), (None, true)));
        });
    }
}
