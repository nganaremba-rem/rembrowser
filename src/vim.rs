//! The vim engine: modes, counts, pending keys, built-in keys, user maps, hints.
//!
//! Keys arrive from `KeyboardHandler::on_pre_key_event` *before* the page sees them;
//! returning true swallows the key. Insert mode is Chromium's own focus state
//! (`focus_on_editable_field`), so typing in any field just works.

use crate::commands::{self, opt, opt_num};
use crate::keys::{self, Key};
use crate::tabs;
use crate::ui::{self, CmdKind};
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
}

#[derive(Clone, Copy, Debug)]
enum Act {
    Scroll(i32, i32),
    Pages(f32),
    Top,
    Bottom,
    Left,
    Right,
    Hint(HintAct, bool),
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
}

/// vimb's default normal-mode keys (`man vimb`), plus Ctrl-T/W/Tab for muscle memory.
const BUILTINS: &[(&str, Act)] = &[
    ("j", Act::Scroll(0, 1)),
    ("k", Act::Scroll(0, -1)),
    ("h", Act::Scroll(-1, 0)),
    ("l", Act::Scroll(1, 0)),
    ("<C-d>", Act::Pages(0.5)),
    ("<C-u>", Act::Pages(-0.5)),
    ("<C-f>", Act::Pages(1.0)),
    ("<C-b>", Act::Pages(-1.0)),
    ("<Space>", Act::Pages(0.9)),
    ("gg", Act::Top),
    ("G", Act::Bottom),
    ("0", Act::Left),
    ("^", Act::Left),
    ("$", Act::Right),
    ("f", Act::Hint(HintAct::Click, false)),
    ("F", Act::Hint(HintAct::BgTab, true)),
    (";o", Act::Hint(HintAct::Click, false)),
    (";t", Act::Hint(HintAct::BgTab, true)),
    (";y", Act::Hint(HintAct::Yank, true)),
    (";Y", Act::Hint(HintAct::YankText, false)),
    (";x", Act::Hint(HintAct::XCommand, true)),
    (";O", Act::Hint(HintAct::PromptOpen, true)),
    (";T", Act::Hint(HintAct::PromptTab, true)),
    ("o", Act::Prompt("open ")),
    ("O", Act::PromptUrl("open ")),
    ("t", Act::Prompt("tabopen ")),
    ("T", Act::PromptUrl("tabopen ")),
    (":", Act::Prompt("")),
    ("/", Act::Search(true)),
    ("?", Act::Search(false)),
    ("n", Act::SearchNext(false)),
    ("N", Act::SearchNext(true)),
    ("y", Act::YankUrl),
    ("p", Act::Paste(false)),
    ("P", Act::Paste(true)),
    ("r", Act::Reload),
    ("R", Act::ReloadNoCache),
    ("<C-c>", Act::Stop),
    ("<C-o>", Act::Back),
    ("<C-i>", Act::Forward),
    ("u", Act::Reopen(false)),
    ("U", Act::Reopen(true)),
    ("gt", Act::TabNext),
    ("gT", Act::TabPrev),
    ("g0", Act::TabFirst),
    ("g$", Act::TabLast),
    ("<C-Tab>", Act::TabNext),
    ("<C-S-Tab>", Act::TabPrev),
    ("<C-t>", Act::NewTab),
    ("<C-w>", Act::CloseTab),
    ("gu", Act::UrlUp),
    ("gU", Act::UrlRoot),
    ("gh", Act::Home(false)),
    ("gH", Act::Home(true)),
    ("gi", Act::FocusInput),
    ("i", Act::FocusInput),
    ("<C-z>", Act::Pass),
    ("zi", Act::Zoom(0.5)),
    ("zo", Act::Zoom(-0.5)),
    ("zz", Act::ZoomReset),
    ("<C-q>", Act::QuitAll),
    ("<Esc>", Act::Escape),
];

struct Map {
    rhs: String,
    noremap: bool,
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
}

thread_local! {
    static VIM: RefCell<Vim> = RefCell::new(Vim {
        mode: Mode::Normal,
        pending: Vec::new(),
        count: None,
        generation: 0,
        builtins: BUILTINS.iter().map(|(k, a)| (keys::parse(k), *a)).collect(),
        maps: HashMap::new(),
        hint_act: HintAct::Click,
        last_hint: String::new(),
        last_cmd: String::new(),
        last_search: None,
    });
}

/// vim's timeoutlen: how long an ambiguous prefix (exact map + longer map) waits.
const TIMEOUT_MS: i64 = 1000;

pub fn map(mode: &str, lhs: &str, rhs: &str, noremap: bool) {
    let m = mode.chars().next().unwrap_or('n');
    VIM.with_borrow_mut(|v| {
        v.maps.entry(m).or_default().insert(keys::parse(lhs), Map { rhs: rhs.into(), noremap });
    });
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
    let extra = format!("{}{pending}", count.map(|c| c.to_string()).unwrap_or_default());
    let (name, color) = match mode {
        Mode::Normal if !extra.is_empty() => (extra.as_str(), ui::BLUE),
        Mode::Normal => ("NORMAL", ui::BLUE),
        Mode::Insert => ("INSERT", 0xFFA6_E3A1),
        Mode::Hint => ("HINT", ui::YELLOW),
        Mode::Pass => ("PASS", ui::MAUVE),
    };
    ui::set_mode(name, color);
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
    let Some(k) = keys::from_event(e) else { return false };
    if std::env::var_os("REMBROWSER_DEBUG_KEYS").is_some() {
        let mode = VIM.with_borrow(|v| v.mode);
        eprintln!("key {k:?} mode={mode:?} editable={}", e.focus_on_editable_field);
    }
    ui::clear_message();
    let editable = e.focus_on_editable_field != 0;
    let mode = VIM.with_borrow_mut(|v| {
        v.generation += 1;
        v.mode
    });
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
const GLOBAL_KEYS: &[&str] = &["<C-Tab>", "<C-S-Tab>", "<C-t>", "<C-q>"];

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
            set_mode(Mode::Normal);
            true
        }
        "<C-z>" => {
            set_mode(Mode::Pass);
            true
        }
        _ => {
            let rhs = VIM.with_borrow(|v| v.maps.get(&'i').and_then(|m| m.get(&vec![k.to_string()])).map(|m| (m.rhs.clone(), m.noremap)));
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
        if v.pending.is_empty() && k.len() == 1 && k.chars().all(|c| c.is_ascii_digit()) && (k != "0" || v.count.is_some()) {
            let d = k.parse::<u32>().unwrap_or(0);
            v.count = Some(v.count.unwrap_or(0).saturating_mul(10).saturating_add(d).min(9999));
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
                let single = v.pending.len() == 1 && v.count.is_none();
                v.pending.clear();
                v.count = None;
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
                ui::cmdline_open(kind, &text);
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
    let scroll = |kind: &str, dx: i32, dy: i32| {
        let smooth = commands::opt_bool("smooth-scrolling", true);
        tabs::js(&format!("__rem.smooth={smooth};__rem.scroll('{kind}',{dx},{dy})"));
    };
    match a {
        Act::Scroll(dx, dy) => scroll("by", dx * step * n, dy * step * n),
        Act::Pages(p) => tabs::js(&format!(
            "__rem.smooth={};__rem.scroll('page',0,{})",
            commands::opt_bool("smooth-scrolling", true),
            p * n as f32
        )),
        // 50G = 50% (vimb), plain G = bottom.
        Act::Top if count > 1 => scroll("percent", 0, n.min(100)),
        Act::Bottom if count > 1 => scroll("percent", 0, n.min(100)),
        Act::Top => scroll("top", 0, 0),
        Act::Bottom => scroll("bottom", 0, 0),
        Act::Left => scroll("left", 0, 0),
        Act::Right => scroll("right", 0, 0),
        Act::Hint(h, links) => start_hints(h, links),
        Act::Prompt(p) => ui::cmdline_open(CmdKind::Ex, p),
        Act::PromptUrl(p) => ui::cmdline_open(CmdKind::Ex, &format!("{p}{}", tabs::active_url())),
        Act::Search(forward) => ui::cmdline_open(CmdKind::Search { forward }, ""),
        Act::SearchNext(reverse) => {
            if let Some((text, forward)) = VIM.with_borrow(|v| v.last_search.clone()) {
                tabs::find(&text, forward != reverse, true);
            }
        }
        Act::YankUrl => yank(&tabs::active_url()),
        Act::Paste(new_tab) => {
            let clip = commands::command("wl-paste").arg("-n").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
            match clip {
                Ok(s) if !s.is_empty() => commands::open(&s, new_tab),
                _ => ui::message("clipboard is empty", true),
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
        Act::FocusInput => tabs::js("__rem.focusInput()"),
        Act::Pass => set_mode(Mode::Pass),
        Act::Zoom(d) => zoom(d * n as f64),
        Act::ZoomReset => tabs::set_zoom(0.0),
        Act::QuitAll => tabs::quit_all(),
        Act::Escape => {
            tabs::stop_find();
            tabs::js("__rem.hintsClear()");
        }
    }
}

pub fn zoom(delta: f64) {
    let level = tabs::zoom() + delta;
    tabs::set_zoom(level);
    ui::message(&format!("zoom {:.0}%", 1.2f64.powf(level) * 100.0), false);
}

fn yank(text: &str) {
    let child = commands::command("wl-copy").arg("--").arg(text).spawn();
    match child {
        Ok(_) => ui::message(&format!("yanked {text}"), false),
        Err(e) => ui::message(&format!("wl-copy: {e}"), true),
    }
}

/// `https://a.com/b/c/?q` → `https://a.com/b/`.
fn url_up(url: &str) -> String {
    let root = url_root(url);
    let base = url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/');
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

/// <C-r>{reg}: % URL, ; last hinted URL, : last command, / last search.
pub fn register(k: &str) -> Option<String> {
    match k {
        "%" => Some(tabs::active_url()),
        ";" => VIM.with_borrow(|v| Some(v.last_hint.clone())),
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

fn start_hints(h: HintAct, links_only: bool) {
    VIM.with_borrow_mut(|v| v.hint_act = h);
    set_mode(Mode::Hint);
    let keys = opt("hint-keys").unwrap_or_else(|| "asdfghjkl".into());
    // JSON-quote the keys: they come from the config file.
    let keys: String = keys.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    tabs::js(&format!("__rem.hints('{keys}',{links_only})"));
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
        ["focus", on] => {
            let mode = VIM.with_borrow(|v| v.mode);
            if matches!(mode, Mode::Normal | Mode::Insert) {
                set_mode(if *on == "1" { Mode::Insert } else { Mode::Normal });
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
        HintAct::PromptOpen => ui::cmdline_open(CmdKind::Ex, &format!("open {href}")),
        HintAct::PromptTab => ui::cmdline_open(CmdKind::Ex, &format!("tabopen {href}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_navigation() {
        assert_eq!(url_up("https://a.com/b/c/?q=1"), "https://a.com/b/");
        assert_eq!(url_up("https://a.com/b"), "https://a.com/");
        assert_eq!(url_up("https://a.com/"), "https://a.com/");
        assert_eq!(url_root("https://a.com/b/c"), "https://a.com/");
    }

    #[test]
    fn user_map_beats_builtin_and_prefixes_wait() {
        map("n", "d", ":q<CR>", true);
        map("n", "<Space>ma", ":bma<CR>", true);
        VIM.with_borrow(|v| {
            assert!(matches!(lookup(v, &keys::parse("d"), true), (Some(Lookup::User(..)), false)));
            assert!(matches!(lookup(v, &keys::parse("<Space>m"), true), (None, true)));
            // <Space> alone is a builtin (page down) but also a prefix: must wait.
            assert!(matches!(lookup(v, &keys::parse("<Space>"), true), (Some(Lookup::Builtin(_)), true)));
            assert!(matches!(lookup(v, &keys::parse("g"), true), (None, true)));
        });
    }
}
