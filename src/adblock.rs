//! Network ad/tracker blocking with Brave's adblock-rust (uBlock Origin syntax).
//!
//! The engine is built on a background thread and cached serialized in
//! $XDG_CACHE_HOME/rembrowser/engine.dat, so later launches load it in milliseconds.
//! Requests made before it's ready pass through.

use adblock::{
    Engine,
    lists::{FilterSet, ParseOptions},
    request::Request,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, SystemTime},
};

static ENGINE: OnceLock<Engine> = OnceLock::new();

const LISTS: &[(&str, &str)] = &[
    ("easylist", "https://easylist.to/easylist/easylist.txt"),
    (
        "easyprivacy",
        "https://easylist.to/easylist/easyprivacy.txt",
    ),
    (
        "ubo-filters",
        "https://ublockorigin.github.io/uAssets/filters/filters.txt",
    ),
    (
        "ubo-privacy",
        "https://ublockorigin.github.io/uAssets/filters/privacy.txt",
    ),
    (
        "ubo-badware",
        "https://ublockorigin.github.io/uAssets/filters/badware.txt",
    ),
    (
        "ubo-quick-fixes",
        "https://ublockorigin.github.io/uAssets/filters/quick-fixes.txt",
    ),
    (
        "peter-lowe",
        "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=adblockplus&showintro=0&mimetype=plaintext",
    ),
];

/// uBO refreshes its lists every ~4 days.
const MAX_AGE: Duration = Duration::from_secs(4 * 24 * 3600);

fn cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .unwrap_or_else(|_| format!("{}/.cache", std::env::var("HOME").unwrap_or_default()));
    PathBuf::from(base).join("rembrowser")
}

fn age(p: &Path) -> Option<Duration> {
    SystemTime::now()
        .duration_since(fs::metadata(p).ok()?.modified().ok()?)
        .ok()
}

/// Write via a temp file + rename so a crash never leaves a half-written cache.
fn write_atomic(p: &Path, data: &[u8]) {
    let tmp = p.with_extension("tmp");
    if fs::write(&tmp, data).is_ok() {
        let _ = fs::rename(tmp, p);
    }
}

pub fn start() {
    std::thread::spawn(|| {
        let dat = cache_dir().join("engine.dat");
        // Fast path: any cached engine is used right away, even a stale one.
        if let Some(e) = fs::read(&dat).ok().and_then(|b| {
            let mut e = Engine::default();
            e.deserialize(&b).ok().map(|_| e)
        }) {
            let _ = ENGINE.set(e);
        }
        // Rebuild when stale or missing. A rebuilt engine takes effect next launch
        // unless none was loaded yet.
        // ponytail: no hot-swap; swap via RwLock<Arc<Engine>> if long sessions need fresh lists.
        if age(&dat).is_none_or(|a| a > MAX_AGE) {
            if let Some(e) = build(&dat) {
                let _ = ENGINE.set(e);
            }
        }
    });
}

fn build(dat: &Path) -> Option<Engine> {
    let dir = cache_dir().join("lists");
    fs::create_dir_all(&dir).ok()?;
    let mut set = FilterSet::new(false);
    let mut any = false;
    for (name, url) in LISTS {
        let file = dir.join(format!("{name}.txt"));
        let tmp = file.with_extension("tmp");
        // curl instead of an HTTP crate: it's on every Arch box and keeps the binary small.
        let ok = crate::commands::command("curl")
            .args(["-fsSL", "--max-time", "30", "-o"])
            .arg(&tmp)
            .arg(url)
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            let _ = fs::rename(&tmp, &file);
        }
        // A failed download falls back to the previous copy of that list.
        if let Ok(text) = fs::read_to_string(&file) {
            set.add_filter_list(text, ParseOptions::default());
            any = true;
        }
    }
    if !any {
        eprintln!("rembrowser: adblock: no filter lists available (offline?)");
        return None;
    }
    let engine = Engine::new_with_filter_set(set);
    write_atomic(dat, &engine.serialize());
    Some(engine)
}

/// `rtype` uses adblock's request type names ("script", "image", "sub_frame", ...).
pub fn should_block(url: &str, source: &str, rtype: &str) -> bool {
    let Some(engine) = ENGINE.get() else {
        return false;
    };
    Request::new(url, source, rtype, "GET")
        .is_ok_and(|r| engine.check_network_request(&r).should_block())
}

/// One rule per selector: a selector the browser can't parse then drops only itself.
fn hide_css(selectors: impl IntoIterator<Item = String>) -> String {
    selectors
        .into_iter()
        .map(|s| format!("{s}{{display:none!important}}\n"))
        .collect()
}

/// Site-specific `##` CSS for `url`, and whether generic rules apply on it.
pub fn page_css(url: &str) -> (String, bool) {
    let Some(engine) = ENGINE.get() else {
        return (String::new(), false);
    };
    let r = engine.url_cosmetic_resources(url);
    (hide_css(r.hide_selectors), !r.generichide)
}

/// Generic `##.class` / `###id` CSS for class and id names page.js found on `url`.
pub fn class_id_css(url: &str, classes: &str, ids: &str) -> String {
    let Some(engine) = ENGINE.get() else {
        return String::new();
    };
    let r = engine.url_cosmetic_resources(url);
    if r.generichide {
        return String::new();
    }
    let words = |s: &str| {
        s.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect::<Vec<_>>()
    };
    hide_css(engine.hidden_class_id_selectors(words(classes), words(ids), &r.exceptions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_tracker_but_not_page() {
        let mut set = FilterSet::new(false);
        set.add_filter_list("||doubleclick.net^\n".into(), ParseOptions::default());
        let engine = Engine::new_with_filter_set(set);
        let _ = ENGINE.set(engine);
        assert!(should_block(
            "https://ad.doubleclick.net/x.js",
            "https://example.com/",
            "script"
        ));
        assert!(!should_block(
            "https://example.com/app.js",
            "https://example.com/",
            "script"
        ));
    }

    #[test]
    fn cosmetic_css() {
        let mut set = FilterSet::new(false);
        set.add_filter_list(
            "example.com##.promo\n##.ad-banner\n".into(),
            ParseOptions::default(),
        );
        let engine = Engine::new_with_filter_set(set);
        let r = engine.url_cosmetic_resources("https://example.com/");
        assert_eq!(
            hide_css(r.hide_selectors),
            ".promo{display:none!important}\n"
        );
        let generic = engine.hidden_class_id_selectors(
            ["ad-banner", "content"],
            [] as [&str; 0],
            &r.exceptions,
        );
        assert_eq!(generic, vec![".ad-banner".to_string()]);
    }
}

/// CEF glue: called on the IO thread once per request.
pub mod hook {
    use super::should_block;
    use cef::*;

    wrap_resource_request_handler! {
        pub struct RemResourceRequestHandler;

        impl ResourceRequestHandler {
            // IO thread, once per request: keep it cheap.
            fn on_before_resource_load(
                &self,
                _browser: Option<&mut Browser>,
                frame: Option<&mut Frame>,
                request: Option<&mut Request>,
                _callback: Option<&mut Callback>,
            ) -> ReturnValue {
                let Some(req) = request else { return ReturnValue::CONTINUE };
                let Some(rtype) = adblock_type(req.resource_type()) else { return ReturnValue::CONTINUE };
                let url = CefString::from(&req.url()).to_string();
                let source = frame
                    .map(|f| CefString::from(&f.url()).to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| CefString::from(&req.referrer_url()).to_string());
                if should_block(&url, &source, rtype) {
                    ReturnValue::CANCEL
                } else {
                    ReturnValue::CONTINUE
                }
            }

            // The homescreen is served from here, before any network lookup.
            fn resource_handler(
                &self,
                _browser: Option<&mut Browser>,
                _frame: Option<&mut Frame>,
                request: Option<&mut Request>,
            ) -> Option<ResourceHandler> {
                let url = CefString::from(&request?.url()).to_string();
                crate::home::is_home(&url).then(crate::home::handler)
            }
        }
    }

    /// CEF resource type → adblock request type. None = never block (top-level navigations).
    fn adblock_type(t: ResourceType) -> Option<&'static str> {
        Some(match t {
            ResourceType::MAIN_FRAME => return None,
            ResourceType::SUB_FRAME => "sub_frame",
            ResourceType::STYLESHEET => "stylesheet",
            ResourceType::SCRIPT => "script",
            ResourceType::IMAGE | ResourceType::FAVICON => "image",
            ResourceType::FONT_RESOURCE => "font",
            ResourceType::OBJECT | ResourceType::PLUGIN_RESOURCE => "object",
            ResourceType::MEDIA => "media",
            ResourceType::XHR => "xmlhttprequest",
            ResourceType::PING | ResourceType::CSP_REPORT => "ping",
            _ => "other",
        })
    }
}
