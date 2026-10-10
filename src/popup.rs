//! noice-style command popup: popup.html in an overlay BrowserView floating over the
//! page, sized to exactly its box (windowed CEF views can't be transparent). Also asks
//! permission questions.
//!
//! Rust → page: `rem.open(state)`, `rem.count(text)`. Page → Rust: console messages
//! "\x01pop\t<type>\t…" on this view's own client, so web pages can't fake them.

use crate::{commands, editor, permissions, tabs, ui, vim};
use cef::*;
use std::cell::RefCell;

const HTML: &str = include_str!("popup.html");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmdKind {
    Ex,
    Search { forward: bool },
}

/// Which window a popup floats over.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Owner {
    /// A browser window, by its id (tabs.rs).
    Win(u32),
    /// A web-app window, by its page's browser id.
    App(i32),
}

struct Pop {
    owner: Owner,
    win: Window,
    bv: BrowserView,
    ctl: OverlayController,
    ready: bool,
    /// Box height reported by popup.html ('size').
    height: i32,
    /// JS to run once popup.html has loaded (a `:` pressed during startup).
    pending: Option<String>,
}

type OnPick = Box<dyn FnOnce(usize)>;
type OnConfirm = Box<dyn FnOnce(bool)>;
type OnAct = Box<dyn Fn(&str, usize)>;
type OnForm = Box<dyn FnOnce(Vec<String>)>;

/// A picker row action: (key in popup notation, label, name handed to `on_act`).
pub type Action = (&'static str, &'static str, &'static str);
pub const EDIT: Action = ("C-e", "edit", "edit");
pub const DELETE: Action = ("C-x", "delete", "delete");

thread_local! {
    /// One popup per window (browser window + each web app that used one).
    static POPS: RefCell<Vec<Pop>> = const { RefCell::new(Vec::new()) };
    static ON_PICK: RefCell<Option<OnPick>> = const { RefCell::new(None) };
    static ON_CONFIRM: RefCell<Option<OnConfirm>> = const { RefCell::new(None) };
    static ON_ACT: RefCell<Option<OnAct>> = const { RefCell::new(None) };
    static ON_FORM: RefCell<Option<OnForm>> = const { RefCell::new(None) };
    static HISTORY: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// (name, description) for completion. Mirrors commands::exec_one.
const COMMANDS: &[(&str, &str)] = &[
    ("open", "open URL or search"),
    ("tabopen", "open in new tab"),
    ("winopen", "open in new window"),
    ("quit", "close tab"),
    ("quitall", "quit browser"),
    ("tabs", "pick a tab"),
    ("buffer", "go to tab N"),
    ("back", "history back"),
    ("forward", "history forward"),
    ("reload", "reload (! = no cache)"),
    ("set", "change a setting"),
    ("nnoremap", "map normal-mode keys"),
    ("normal", "run normal-mode keys"),
    ("sh!", "run shell command"),
    ("shellex", "run command, execute its output"),
    ("eval", "run JavaScript"),
    ("bma", "bookmark this page"),
    ("qpush", "read later: queue page"),
    ("qpop", "read later: open oldest"),
    ("source", "run a config file"),
    ("zoom", "zoom by N steps"),
    ("echo", "show a message"),
    ("history", "recent pages (open, delete)"),
    ("bookmarks", "bookmarks (open, edit, delete)"),
    ("keys", "keymap cheatsheet"),
    ("pins", "harpoon pins"),
    ("pin", "pin page: add | N"),
    ("downloads", "downloads (open, show folder, remove)"),
    ("downloads-clear", "forget the download list"),
    ("queue", "read-later list"),
    ("history-clear", "forget all history"),
    ("permissions", "site permissions you always allow / never allow"),
    ("pip", "picture-in-picture"),
    ("mpv", "play this page (or URL) in mpv"),
    ("app", "open this page (or URL) as a web app"),
    ("app-install", "add this page to the app launcher"),
    ("config", "edit config.yaml"),
    ("settings", "Chromium settings (own window)"),
    ("extensions", "Chrome extensions (own window)"),
    ("chrome", "this page (or URL) in a Chromium window with toolbar"),
    ("userscripts", "reload userscripts"),
];

fn with_owner<R>(o: Owner, f: impl FnOnce(&mut Pop) -> R) -> Option<R> {
    POPS.with_borrow_mut(|p| p.iter_mut().find(|p| p.owner == o).map(f))
}

/// The popup of the window keys are going to (focused web app, else the browser).
fn with_cur<R>(f: impl FnOnce(&mut Pop) -> R) -> Option<R> {
    with_owner(tabs::current_owner(), f)
}

fn with_from<R>(b: &Browser, f: impl FnOnce(&mut Pop) -> R) -> Option<R> {
    POPS.with_borrow_mut(|p| {
        p.iter_mut()
            .find(|p| {
                p.bv.browser()
                    .is_some_and(|x| x.is_same(Some(&mut b.clone())) != 0)
            })
            .map(f)
    })
}

pub fn init(window: &Window, owner: Owner) {
    if with_owner(owner, |_| ()).is_some() {
        return;
    }
    let url = format!(
        "data:text/html;base64,{}",
        CefString::from(&base64_encode(Some(HTML.as_bytes())))
    );
    let settings = BrowserSettings {
        background_color: 0xFF1E_1E2E,
        ..Default::default()
    };
    let mut client = PopClient::new();
    let mut delegate = PopViewDelegate::new();
    let Some(bv) = browser_view_create(
        Some(&mut client),
        Some(&url.as_str().into()),
        Some(&settings),
        None,
        None,
        Some(&mut delegate),
    ) else {
        return;
    };
    let Some(ctl) = window.add_overlay_view(Some(&mut View::from(&bv)), DockingMode::CUSTOM, 1)
    else {
        return;
    };
    // The browser only starts once its view is visible: keep it 1px until "ready".
    ctl.set_bounds(Some(&Rect {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    }));
    ctl.set_visible(1);
    POPS.with_borrow_mut(|p| {
        p.push(Pop {
            owner,
            win: window.clone(),
            bv,
            ctl,
            ready: false,
            height: 48,
            pending: None,
        })
    });
}

pub fn js_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn js_list<T>(xs: &[T], f: impl Fn(&T) -> String) -> String {
    format!("[{}]", xs.iter().map(f).collect::<Vec<_>>().join(","))
}

/// History (newest first, deduped) then bookmarks, from vimb's data files.
pub fn urls() -> Vec<(&'static str, String, String)> {
    let read = |f: &str| std::fs::read_to_string(commands::data_dir().join(f)).unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in read("bookmark").lines() {
        let mut p = line.split('\t');
        if let Some(u) = p
            .next()
            .filter(|u| !u.is_empty() && seen.insert(u.to_string()))
        {
            out.push((
                "bookmark",
                p.next().unwrap_or("").to_string(),
                u.to_string(),
            ));
        }
    }
    for line in read("history").lines().rev() {
        let mut p = line.split('\t');
        if let Some(u) = p
            .next()
            .filter(|u| !u.is_empty() && seen.insert(u.to_string()))
        {
            out.push(("history", p.next().unwrap_or("").to_string(), u.to_string()));
            // ponytail: cap keeps the payload small; page the file if history grows huge.
            if out.len() >= 3000 {
                break;
            }
        }
    }
    out
}

fn state_json(kind: CmdKind, text: &str) -> String {
    let (k, forward) = match kind {
        CmdKind::Ex => ("ex", true),
        CmdKind::Search { forward } => ("search", forward),
    };
    let reg = |r: &str| js_str(&vim::register(r).unwrap_or_default());
    let tabs = tabs::titles();
    let tabs: Vec<(usize, String, String)> = tabs
        .into_iter()
        .enumerate()
        .map(|(i, (t, u))| (i + 1, t, u))
        .collect();
    format!(
        "{{kind:{},forward:{forward},text:{},history:{},commands:{},settings:{},urls:{},tabs:{},regs:{{'%':{},';':{},':':{},'/':{}}}}}",
        js_str(k),
        js_str(text),
        HISTORY.with_borrow(|h| js_list(h, |s| js_str(s))),
        js_list(COMMANDS, |(n, d)| format!("[{},{}]", js_str(n), js_str(d))),
        js_list(&commands::settings(), |(n, v)| format!(
            "[{},{}]",
            js_str(n),
            js_str(v)
        )),
        js_list(&urls(), |(k, t, u)| format!(
            "[{},{},{}]",
            js_str(k),
            js_str(t),
            js_str(u)
        )),
        js_list(&tabs, |(i, t, u)| format!(
            "[{i},{},{}]",
            js_str(t),
            js_str(u)
        )),
        reg("%"),
        reg(";"),
        reg(":"),
        reg("/"),
    )
}

/// Opens on the next UI tick: the key that triggered it (`:`, `o`) is still being
/// dispatched, and Views would hand that same press to the newly focused popup.
pub fn open(kind: CmdKind, text: &str) {
    let js = format!("rem.open({})", state_json(kind, text));
    ui::run_on_ui(0, move || show(js));
}

/// Fuzzy picker: rows are (kind icon, title, detail); `on_pick` gets the row index.
pub fn pick(title: &str, rows: &[(&str, String, String)], on_pick: impl FnOnce(usize) + 'static) {
    pick_with(title, rows, &[], on_pick, |_, _| {});
}

/// A picker whose rows can also be edited, deleted, ...: `on_act` gets the action's
/// name and the row index (keys, and buttons on the row under the mouse).
pub fn pick_with(
    title: &str,
    rows: &[(&str, String, String)],
    actions: &[Action],
    on_pick: impl FnOnce(usize) + 'static,
    on_act: impl Fn(&str, usize) + 'static,
) {
    if rows.is_empty() {
        ui::message(&format!("{title}: nothing here"), true);
        return;
    }
    ON_PICK.with_borrow_mut(|p| *p = Some(Box::new(on_pick)));
    ON_ACT.with_borrow_mut(|p| *p = Some(Box::new(on_act)));
    let items = js_list(rows, |(k, t, u)| {
        format!("[{},{},{}]", js_str(k), js_str(t), js_str(u))
    });
    let actions = js_list(actions, |(key, label, name)| {
        format!("[{},{},{}]", js_str(key), js_str(label), js_str(name))
    });
    let js = format!(
        "rem.open({{kind:'pick',title:{},items:{items},actions:{actions},text:'',history:[]}})",
        js_str(title)
    );
    ui::run_on_ui(0, move || show(js));
}

/// A few labelled text fields (label, current value); Enter hands back the values
/// in the same order, with tabs and newlines turned into spaces.
pub fn form(title: &str, fields: &[(&str, &str)], on_submit: impl FnOnce(Vec<String>) + 'static) {
    ON_FORM.with_borrow_mut(|f| *f = Some(Box::new(on_submit)));
    let js = format!(
        "rem.open({{kind:'form',title:{},fields:{}}})",
        js_str(title),
        js_list(fields, |(l, v)| format!("[{},{}]", js_str(l), js_str(v)))
    );
    ui::run_on_ui(0, move || show(js));
}

/// Permission question; the answer comes back through `permissions::answer`.
pub fn ask(id: u64, origin: &str, what: &str) {
    let js = format!(
        "rem.open({{kind:'prompt',id:{id},origin:{},what:{}}})",
        js_str(origin),
        js_str(what)
    );
    ui::run_on_ui(0, move || show(js));
}

/// A yes/no question (y / Enter = yes; n / Esc = no). `yes` labels the yes button.
pub fn confirm(text: &str, yes: &str, on_answer: impl FnOnce(bool) + 'static) {
    // A question still open is answered "no" by the new one.
    if let Some(old) = ON_CONFIRM.with_borrow_mut(|c| c.replace(Box::new(on_answer))) {
        old(false);
    }
    let js = format!(
        "rem.open({{kind:'prompt',confirm:true,msg:{},buttons:[['y',{},'yes'],['n','Cancel','no']]}})",
        js_str(text),
        js_str(yes)
    );
    ui::run_on_ui(0, move || show(js));
}

/// Centred horizontally, upper fifth of the window, as tall as the box.
fn place(p: &Pop) {
    let size = View::from(&p.win).size();
    let width = (size.width as f32 * 0.6).clamp(420.0, 820.0) as i32;
    let y = (size.height as f32 * 0.18) as i32;
    let height = p.height.clamp(20, (size.height - y - 20).max(20));
    p.ctl.set_bounds(Some(&Rect {
        x: (size.width - width) / 2,
        y,
        width,
        height,
    }));
}

fn show(js: String) {
    let owner = tabs::current_owner();
    let Some(win) = tabs::current_window() else {
        return;
    };
    let ready = with_owner(owner, |p| {
        place(p);
        p.ctl.set_visible(1);
        p.bv.request_focus();
        if !p.ready {
            p.pending = Some(js.clone());
        }
        p.ready
    });
    match ready {
        Some(true) => run_js(owner, &js),
        Some(false) => {}
        // Not created yet (`:` before the first page finished, or a web app's first):
        // create it now; it runs `js` once loaded.
        None => {
            init(&win, owner);
            with_owner(owner, |p| p.pending = Some(js));
        }
    }
    ui::set_mode("COMMAND", ui::PEACH);
}

fn run_js(owner: Owner, js: &str) {
    let frame = with_owner(owner, |p| p.bv.browser()?.main_frame()).flatten();
    if let Some(f) = frame {
        f.execute_java_script(Some(&js.into()), Some(&"rembrowser://popup".into()), 0);
    }
}

/// That window closed: its popup went with it.
pub fn forget(owner: Owner) {
    POPS.with_borrow_mut(|p| p.retain(|p| p.owner != owner));
}

/// The popup browser of a window, for that window's can_close.
pub fn host(owner: Owner) -> Option<BrowserHost> {
    with_owner(owner, |p| p.bv.browser()?.host()).flatten()
}

pub fn is_open() -> bool {
    with_cur(|p| p.ctl.is_visible() != 0 && p.ready).unwrap_or(false)
}

fn hide(b: &Browser) {
    with_from(b, |p| p.ctl.set_visible(0));
    tabs::focus_active();
    vim::reset();
}

pub fn set_count(text: &str) {
    if is_open() {
        run_js(
            tabs::current_owner(),
            &format!("rem.count({})", js_str(text)),
        );
    }
}

fn on_message(b: &Browser, fields: &[&str]) {
    match fields {
        ["ready"] => {
            let Some((owner, pending)) = with_from(b, |p| {
                p.ready = true;
                let pending = p.pending.take();
                if pending.is_none() {
                    p.ctl.set_visible(0);
                } else {
                    // Opened while loading: it has the screen, give it the keyboard too.
                    p.bv.request_focus();
                }
                (p.owner, pending)
            }) else {
                return;
            };
            match pending {
                Some(js) => run_js(owner, &js),
                // Loading it took the keyboard from the page; give it back.
                None => tabs::focus_owner(owner),
            }
        }
        ["size", h] => {
            with_from(b, |p| {
                if let Ok(h) = h.parse() {
                    p.height = h;
                    if p.ctl.is_visible() != 0 {
                        place(p);
                    }
                }
            });
        }
        ["submit", kind, forward, text] => {
            hide(b);
            let kind = parse_kind(kind, forward);
            if kind == CmdKind::Ex && !text.trim().is_empty() {
                HISTORY.with_borrow_mut(|h| {
                    h.retain(|x| x != text);
                    h.push(text.to_string());
                });
            }
            vim::cmdline_submit(kind, text);
        }
        ["picked", i] => {
            hide(b);
            let cb = ON_PICK.with_borrow_mut(Option::take);
            if let (Some(cb), Ok(i)) = (cb, i.parse::<usize>()) {
                cb(i);
            }
        }
        ["act", name, i] => {
            hide(b);
            // Taken out while it runs: an action usually opens the picker again.
            let cb = ON_ACT.with_borrow_mut(Option::take);
            if let (Some(cb), Ok(i)) = (cb, i.parse::<usize>()) {
                cb(name, i);
            }
        }
        ["form", values @ ..] => {
            hide(b);
            if let Some(cb) = ON_FORM.with_borrow_mut(Option::take) {
                cb(values.iter().map(|v| v.to_string()).collect());
            }
        }
        ["cancel", kind] => {
            hide(b);
            vim::cmdline_cancel(parse_kind(kind, "1"));
        }
        ["preview", text, forward] => vim::search_preview(text, *forward == "1"),
        ["edit", kind, text] => {
            hide(b);
            edit_in_nvim(parse_kind(kind, "1"), text);
        }
        ["answer", id, choice] => {
            hide(b);
            if let Ok(id) = id.parse() {
                permissions::answer(id, choice);
            }
        }
        ["confirm", choice] => {
            hide(b);
            if let Some(cb) = ON_CONFIRM.with_borrow_mut(Option::take) {
                cb(*choice == "yes");
            }
        }
        _ => {}
    }
}

fn parse_kind(kind: &str, forward: &str) -> CmdKind {
    match kind {
        "search" => CmdKind::Search {
            forward: forward == "1",
        },
        _ => CmdKind::Ex,
    }
}

/// Ctrl-F / Ctrl-T: vim's command-line window. Edit the line in the nvim-vimb popup
/// (niri rule ^vimb-cmdedit$ floats it); write = run it, :q = cancel.
fn edit_in_nvim(kind: CmdKind, text: &str) {
    let cmd = "foot --app-id vimb-cmdedit env NVIM_APPNAME=nvim-vimb VIMB_POPUP=cmdline nvim %s";
    editor::edit(&format!("{text}\n"), cmd, move |text| {
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            vim::cmdline_submit(kind, line);
        }
    });
}

wrap_browser_view_delegate! {
    struct PopViewDelegate {}

    impl ViewDelegate {}

    impl BrowserViewDelegate {
        fn browser_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::ALLOY
        }
    }
}

wrap_client! {
    struct PopClient;

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(PopDisplayHandler::new())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(PopLifeSpanHandler::new())
        }
    }
}

// Only closed by the window closing (tabs::can_close). Like a tab: remove the view
// ourselves (the default would close the window mid-close), then retry the window
// close, which now finds the popup gone.
wrap_life_span_handler! {
    struct PopLifeSpanHandler;

    impl LifeSpanHandler {
        fn do_close(&self, browser: Option<&mut Browser>) -> i32 {
            let Some(b) = browser else { return 1 };
            let owner = POPS.with_borrow_mut(|pops| {
                let i = pops.iter().position(|p| p.bv.browser().is_some_and(|x| x.is_same(Some(&mut b.clone())) != 0))?;
                let p = pops.remove(i);
                p.ctl.destroy();
                Some(p.owner)
            });
            // Retry closing that window, which now finds its popup gone.
            match owner {
                Some(Owner::App(id)) => ui::run_on_ui(0, move || crate::apps::close(id)),
                Some(Owner::Win(id)) => ui::run_on_ui(0, move || tabs::close_window(id)),
                None => {}
            }
            1
        }
    }
}

wrap_display_handler! {
    struct PopDisplayHandler;

    impl DisplayHandler {
        fn on_console_message(
            &self,
            browser: Option<&mut Browser>,
            _level: LogSeverity,
            message: Option<&CefString>,
            _source: Option<&CefString>,
            _line: i32,
        ) -> i32 {
            let msg = message.map(|m| m.to_string()).unwrap_or_default();
            if let Some(body) = msg.strip_prefix("\u{1}pop\t") {
                let fields: Vec<&str> = body.split('\t').collect();
                if let Some(b) = browser {
                    on_message(b, &fields);
                }
                return 1;
            }
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::js_str;

    #[test]
    fn js_strings_are_escaped() {
        assert_eq!(js_str(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(js_str("</script>"), r#""\u003c/script>""#);
        assert_eq!(js_str("x\ny"), r#""x\u000ay""#);
    }
}
