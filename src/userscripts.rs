//! Userscripts — rembrowser's extension story (CEF's Alloy runtime can't load Chrome
//! extensions). Greasemonkey-style files in ~/.config/rembrowser/userscripts/*.js:
//!
//!   // ==UserScript==
//!   // @match   *://*.youtube.com/*
//!   // @run-at  document-start | document-end (default) | document-idle
//!   // ==/UserScript==
//!
//! @match / @include / @exclude take `*` globs; no @match/@include = every page.
//! Provided: GM_addStyle(css) (CSP-proof), GM_info. Not provided: GM_xmlhttpRequest.

use std::cell::RefCell;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RunAt {
    Start,
    End,
}

struct Script {
    name: String,
    include: Vec<String>,
    exclude: Vec<String>,
    run_at: RunAt,
    code: String,
}

thread_local! {
    static SCRIPTS: RefCell<Vec<Script>> = const { RefCell::new(Vec::new()) };
}

fn dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map_or_else(
            |_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"),
            PathBuf::from,
        )
        .join("rembrowser/userscripts")
}

/// (Re)load every script; returns how many.
pub fn load() -> usize {
    let mut scripts = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir()) {
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "js"))
            .collect();
        paths.sort();
        for p in paths {
            if let Ok(code) = std::fs::read_to_string(&p) {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                scripts.push(parse(name, code));
            }
        }
    }
    let n = scripts.len();
    SCRIPTS.with_borrow_mut(|s| *s = scripts);
    n
}

fn parse(name: String, code: String) -> Script {
    let (mut include, mut exclude, mut run_at) = (Vec::new(), Vec::new(), RunAt::End);
    let mut in_meta = false;
    for line in code.lines() {
        let l = line.trim();
        if l.starts_with("// ==UserScript==") {
            in_meta = true;
        } else if l.starts_with("// ==/UserScript==") {
            break;
        } else if in_meta {
            let mut parts = l.trim_start_matches("//").split_whitespace();
            match (parts.next(), parts.next()) {
                (Some("@match" | "@include"), Some(p)) => include.push(p.to_string()),
                (Some("@exclude" | "@exclude-match"), Some(p)) => exclude.push(p.to_string()),
                (Some("@run-at"), Some(r)) => {
                    run_at = if r == "document-start" {
                        RunAt::Start
                    } else {
                        RunAt::End
                    }
                }
                _ => {}
            }
        }
    }
    Script {
        name,
        include,
        exclude,
        run_at,
        code,
    }
}

/// `*` matches any run of characters (including none); everything else is literal.
fn glob(pat: &str, s: &str) -> bool {
    let (p, s) = (pat.as_bytes(), s.as_bytes());
    let (mut pi, mut si, mut star, mut mark) = (0, 0, None, 0);
    while si < s.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if pi < p.len() && p[pi] == s[si] {
            pi += 1;
            si += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

/// Chrome match patterns: `*://*.example.com/*` also matches example.com itself.
fn matches(pat: &str, url: &str) -> bool {
    pat == "<all_urls>"
        || glob(pat, url)
        || (pat.contains("://*.") && glob(&pat.replacen("://*.", "://", 1), url))
}

fn wanted(s: &Script, url: &str) -> bool {
    (s.include.is_empty() || s.include.iter().any(|p| matches(p, url)))
        && !s.exclude.iter().any(|p| matches(p, url))
}

/// JS for the scripts that run on `url` at `at`, each isolated in its own function
/// so one broken script can't stop the others.
pub fn for_url(url: &str, at: RunAt) -> Option<String> {
    if !url.starts_with("http") && !url.starts_with("file:") {
        return None;
    }
    let js: String = SCRIPTS.with_borrow(|scripts| {
        scripts
            .iter()
            .filter(|s| s.run_at == at && wanted(s, url))
            .map(|s| {
                format!(
                    "try {{ (function (GM_info) {{\n{}\n}})({{ script: {{ name: {} }} }}); }} catch (e) {{ console.error('userscript {}:', e); }}\n",
                    s.code,
                    crate::popup::js_str(&s.name),
                    s.name.replace(['\'', '\\', '\n'], "")
                )
            })
            .collect()
    });
    (!js.is_empty()).then(|| format!("{GM_SHIM}\n{js}"))
}

/// GM_addStyle via adoptedStyleSheets: works under strict CSP, before <head> exists.
const GM_SHIM: &str = "window.GM_addStyle ??= (css) => { const s = new CSSStyleSheet(); s.replaceSync(css); \
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, s]; return s; };";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_and_match_patterns() {
        assert!(glob("https://*.example.com/*", "https://a.example.com/x"));
        assert!(!glob("https://*.example.com/*", "https://example.org/"));
        assert!(matches(
            "*://*.youtube.com/*",
            "https://youtube.com/watch?v=1"
        ));
        assert!(matches("*://*.youtube.com/*", "https://www.youtube.com/"));
        assert!(!matches("*://*.youtube.com/*", "https://notyoutube.org/"));
        assert!(matches("<all_urls>", "https://x.y/"));
    }

    #[test]
    fn parses_metadata() {
        let s = parse(
            "t.js".into(),
            "// ==UserScript==\n// @match *://a.com/*\n// @exclude *://a.com/skip*\n// @run-at document-start\n// ==/UserScript==\nx()".into(),
        );
        assert_eq!(s.run_at, RunAt::Start);
        assert!(wanted(&s, "https://a.com/page"));
        assert!(!wanted(&s, "https://a.com/skip/1"));
        assert!(!wanted(&s, "https://b.com/"));
    }
}
