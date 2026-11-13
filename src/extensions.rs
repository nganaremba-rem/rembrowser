//! Optional Chrome extensions (`extensions: true`).
//!
//! The Web Store only installs into Chrome ("Switch to Chrome"), so `:ext-install`
//! fetches the extension's package from Google's download service itself — the file
//! Chrome would get — and unpacks it to ~/.local/share/rembrowser/extensions/<id>.
//! Every folder there is handed to Chromium with --load-extension at startup, so
//! installs and removals apply after a restart.
//! `:extensions` lists them; the one picked opens the page behind its toolbar button in
//! a small window. `:ext-manage` is chrome://extensions (options, on/off switches).
//!
//! ponytail: loaded unpacked, an extension without a "key" in its manifest gets its
//! id from its folder path, not from the store (`runtime_id`). Write the package's
//! public key into manifest.json if one ever needs its store id. No auto-update
//! either: `:ext-install` again fetches the latest.

use crate::{commands, popup, tabs, ui, widevine};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The real profile: a private window (`--private`) has a profile of its own, but
/// runs the same extensions, loaded unpacked from the real folders.
fn real_data() -> PathBuf {
    PathBuf::from(crate::real_data_dir())
}

fn dir() -> PathBuf {
    real_data().join("extensions")
}

/// Web Store installs can't be loaded unpacked as they are (their `_metadata` signature
/// folder is refused), so a private window runs a copy without it.
fn private_copy(folder: &Path, id: &str) -> PathBuf {
    let dest = PathBuf::from(crate::data_dir()).join("ext-store").join(id);
    if !dest.exists() {
        let _ = std::fs::create_dir_all(dest.parent().unwrap_or(&dest));
        let copied = commands::command("cp").arg("-a").arg(folder).arg(&dest).status();
        if copied.is_ok_and(|s| s.success()) {
            let _ = std::fs::remove_dir_all(dest.join("_metadata"));
        }
    }
    dest
}

/// The extension id (32 letters a–p) in a Web Store URL, or given as is.
fn id_in(text: &str) -> Option<&str> {
    text.split(|c: char| !c.is_ascii_lowercase())
        .find(|w| w.len() == 32 && w.bytes().all(|b| (b'a'..=b'p').contains(&b)))
}

fn json(path: PathBuf) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The extension's name; "__MSG_key__" names live in the default locale's messages.
fn name_of(dir: &Path) -> Option<String> {
    let manifest = json(dir.join("manifest.json"))?;
    let name = manifest["name"].as_str()?;
    let Some(key) = name.strip_prefix("__MSG_").and_then(|k| k.strip_suffix("__")) else {
        return Some(name.into());
    };
    let locale = manifest["default_locale"].as_str().unwrap_or("en");
    let messages = json(dir.join("_locales").join(locale).join("messages.json"))?;
    let (_, entry) = messages
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))?;
    entry["message"].as_str().map(String::from)
}

/// (id, name) of every installed extension.
fn installed() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            // Not a half-unpacked "<id>.partial".
            (id_in(&id) == Some(id.as_str()) && e.path().join("manifest.json").exists())
                .then(|| (name_of(&e.path()).unwrap_or_else(|| id.clone()), id))
        })
        .map(|(name, id)| (id, name))
        .collect();
    out.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    out
}

/// The tabs here are in no browser window, so an extension sees them wrongly (window id
/// -1, no active tab, no tab events). Three small patches to its files put that right:
/// what each one does is written in the two scripts.
const SHIM: &str = include_str!("ext-shim.js");
/// Goes at the end of the extension's own content script.
const ACTIVE: &str = include_str!("ext-active.js");
const SHIM_END: &str = "/*end rembrowser*/\n";
const SHIM_FILE: &str = "rembrowser-shim.js";

/// `code` without a block of ours (this version or an older one).
fn strip(code: &str) -> String {
    match (code.find("/*rembrowser"), code.find(SHIM_END)) {
        (Some(a), Some(b)) if a < b => format!("{}{}", code[..a].trim_end_matches('\n'), &code[b + SHIM_END.len()..]),
        // The first shim was one line at the top.
        (Some(0), None) => code.split_once('\n').map_or("", |(_, rest)| rest).into(),
        _ => code.into(),
    }
}

/// The page with the shim as its first script. None = it is there already.
fn with_shim_tag(html: &str) -> Option<String> {
    if html.contains(SHIM_FILE) {
        return None;
    }
    let at = html.find("<script").or_else(|| html.find("<head>").map(|i| i + 6)).unwrap_or(0);
    Some(format!("{}<script src=\"/{SHIM_FILE}\"></script>{}", &html[..at], &html[at..]))
}

/// Write `new` to `path` unless that is what it holds. True if the file changed.
fn put(path: &Path, new: &str) -> bool {
    !std::fs::read_to_string(path).is_ok_and(|old| old == new) && std::fs::write(path, new).is_ok()
}

/// Patch one extension (manifest v3). Files, not manifest.json: Chromium loads a store
/// install's manifest from its saved prefs, so a change there is ignored. Idempotent, and
/// swaps an older shim for this one; `:ext-install` unpacks a clean copy, patched again
/// at the next start. True if the service worker changed.
fn patch(folder: &Path) -> bool {
    let Some(manifest) = json(folder.join("manifest.json")) else { return false };
    let Some(sw) = manifest["background"]["service_worker"].as_str() else { return false };
    // 1. The shim at the top of the worker. (A wrapper that `importScripts` the original
    //    fails to start bundles like Bitwarden's.)
    let Ok(code) = std::fs::read_to_string(folder.join(sw)) else { return false };
    let changed = put(&folder.join(sw), &format!("{SHIM}{}", strip(&code)));
    // 2. "I am the active page" at the end of a content script that runs in the top frame.
    let scripts = manifest["content_scripts"].as_array().map(Vec::as_slice).unwrap_or_default();
    let top = scripts.iter().find(|c| c["all_frames"] != true).or(scripts.first());
    if let Some(js) = top.and_then(|c| c["js"][0].as_str()) {
        let path = folder.join(js.trim_start_matches('/'));
        if let Ok(code) = std::fs::read_to_string(&path) {
            put(&path, &format!("{}{ACTIVE}", strip(&code)));
        }
    }
    // 3. The same shim in the popup page: it asks "which tab is active?" too.
    if let Some(page) = popup_page(&manifest) {
        let path = folder.join(page.trim_start_matches('/').split(['?', '#']).next().unwrap_or(""));
        if let Some(html) = std::fs::read_to_string(&path).ok().and_then(|h| with_shim_tag(&h)) {
            let _ = std::fs::write(&path, html);
        }
        put(&folder.join(SHIM_FILE), SHIM);
    }
    changed
}

/// (name, folder, id) of the extensions installed through the Web Store page
/// (Default/Extensions/<id>/<version>, newest version).
fn store_installed() -> Vec<(String, PathBuf, String)> {
    let root = real_data().join("Default/Extensions");
    let mut out = Vec::new();
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let id = e.file_name().to_string_lossy().into_owned();
        let newest = std::fs::read_dir(e.path()).into_iter().flatten().flatten().map(|v| v.path()).max();
        if let Some(folder) = newest.filter(|f| f.join("manifest.json").exists()) {
            if id_in(&id) == Some(id.as_str()) {
                out.push((name_of(&folder).unwrap_or_else(|| id.clone()), folder, id));
            }
        }
    }
    out
}

/// (name, folder, runtime id) of everything the browser runs. An unpacked copy of an
/// extension that is also installed from the Web Store is not run: two of the same
/// extension would both draw their menus on a page.
fn runnable() -> Vec<(String, PathBuf, String)> {
    let mut out = store_installed();
    if crate::is_private() {
        for o in &mut out {
            o.1 = private_copy(&o.1, &o.2);
        }
    }
    let unpacked: Vec<_> = installed()
        .into_iter()
        .filter(|(_, name)| !out.iter().any(|o| &o.0 == name))
        .map(|(id, name)| (name, dir().join(&id), id))
        .collect();
    out.extend(unpacked);
    out.sort_by_key(|o| o.0.to_lowercase());
    out
}

/// (name, folder, id) of extensions added with "Load unpacked" in chrome://extensions:
/// Chromium keeps them in the profile prefs (location 4) and reloads them itself, so
/// they are listed but never patched or passed to --load-extension.
fn loaded_unpacked() -> Vec<(String, PathBuf, String)> {
    let mut out: Vec<(String, PathBuf, String)> = Vec::new();
    for file in ["Preferences", "Secure Preferences"] {
        let prefs = json(real_data().join("Default").join(file)).unwrap_or_default();
        for (id, e) in prefs["extensions"]["settings"].as_object().into_iter().flatten() {
            let folder = PathBuf::from(e["path"].as_str().unwrap_or_default());
            let known = out.iter().any(|o| &o.2 == id);
            if e["location"] == 4 && !known && !folder.starts_with(dir()) && folder.join("manifest.json").exists() {
                out.push((name_of(&folder).unwrap_or_else(|| id.clone()), folder, id.clone()));
            }
        }
    }
    out
}

/// Value for --load-extension: the unpacked folders that are not duplicates (None =
/// nothing to load). Patches every extension's worker first.
pub fn load_switch() -> Option<String> {
    let all = runnable();
    // Chromium runs the worker script it stored last time, however the file changed:
    // after patching, forget the stored workers (sites re-register theirs on the next visit).
    let patched = all.iter().filter(|(_, folder, _)| patch(folder)).count();
    if patched > 0 {
        let _ = std::fs::remove_dir_all(PathBuf::from(crate::data_dir()).join("Default/Service Worker"));
    }
    // Chromium loads Web Store installs from the profile itself; a private profile
    // has none, so there they are loaded like the rest (as copies, see runnable).
    let private = crate::is_private();
    let store = if private { Vec::new() } else { store_installed() };
    let mut dirs: Vec<String> = all
        .iter()
        .filter(|(_, _, id)| !store.iter().any(|s| &s.2 == id))
        .map(|(_, folder, _)| folder.display().to_string())
        .collect();
    if private {
        dirs.extend(loaded_unpacked().into_iter().map(|(_, folder, _)| folder.display().to_string()));
    }
    (!dirs.is_empty()).then(|| dirs.join(","))
}

/// `:ext-install [Web Store URL or id]`; no argument = the store page you are on.
pub fn install(arg: &str) {
    let source = if arg.is_empty() {
        tabs::active_url()
    } else {
        arg.to_string()
    };
    let Some(id) = id_in(&source).map(String::from) else {
        ui::message(
            "ext-install: give a Web Store URL or extension id, or run it on the extension's store page",
            true,
        );
        return;
    };
    ui::message(&format!("downloading extension {id} …"), false);
    std::thread::spawn(move || {
        let result = fetch(&id);
        ui::run_on_ui(0, move || match result {
            Ok(name) => {
                let hint = match commands::opt_bool("extensions", false) {
                    true => "restart to load it",
                    false => "set extensions: true and restart to load it",
                };
                ui::toast(&format!("✓  {name} installed — {hint}"), false);
            }
            Err(e) => ui::message(&format!("ext-install: {e}"), true),
        });
    });
}

fn fetch(id: &str) -> Result<String, String> {
    let url = format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&acceptformat=crx2,crx3\
         &prodversion={}.0.0.0&x=id%3D{id}%26installsource%3Dondemand%26uc",
        cef::sys::CHROME_VERSION_MAJOR
    );
    let crx = widevine::curl(&[&url])?;
    if crx.is_empty() {
        return Err("the Web Store has no such extension (for this Chromium version)".into());
    }
    let zip = widevine::crx3_zip(&crx)?;
    let (dest, tmp) = (dir().join(id), dir().join(format!("{id}.partial")));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    widevine::unzip(zip, &tmp)?;
    if !tmp.join("manifest.json").exists() {
        return Err("no manifest.json in the package".into());
    }
    // The store's signature folder: Chromium refuses unpacked extensions that have it.
    let _ = std::fs::remove_dir_all(tmp.join("_metadata"));
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    Ok(name_of(&dest).unwrap_or_else(|| id.to_string()))
}

/// `:ext-remove`: pick an installed extension to delete.
pub fn remove_picker() {
    let list = installed();
    let rows: Vec<_> = list
        .iter()
        .map(|(id, name)| ("setting", name.clone(), id.clone()))
        .collect();
    let remove = move |i: usize| {
        let (id, name) = &list[i];
        match std::fs::remove_dir_all(dir().join(id)) {
            Ok(()) => ui::message(&format!("removed {name} — gone after a restart"), false),
            Err(e) => ui::message(&format!("ext-remove: {e}"), true),
        }
    };
    let on_act = remove.clone();
    popup::pick_with(
        "\u{f0483} Remove extension",
        &rows,
        &[popup::DELETE],
        remove,
        move |_, i| on_act(i),
    );
}

/// The id Chromium runs the unpacked folder under: the store id when the manifest
/// carries the store's key, else the SHA-256 of the folder's real path, first 16
/// bytes, each hex digit as a letter a–p.
fn runtime_id(folder: &Path, id: &str) -> String {
    if json(folder.join("manifest.json")).is_some_and(|m| m["key"].is_string()) {
        return id.into();
    }
    let real = folder.canonicalize().unwrap_or(folder.to_path_buf());
    id_for_path(&real.to_string_lossy())
}

fn id_for_path(path: &str) -> String {
    Sha256::digest(path.as_bytes())[..16]
        .iter()
        .flat_map(|b| [b >> 4, b & 15])
        .map(|n| (b'a' + n) as char)
        .collect()
}

/// The page behind the toolbar button, else the options page.
fn popup_page(manifest: &serde_json::Value) -> Option<&str> {
    [
        &manifest["action"]["default_popup"],
        &manifest["browser_action"]["default_popup"],
        &manifest["page_action"]["default_popup"],
        &manifest["options_ui"]["page"],
        &manifest["options_page"],
    ]
    .into_iter()
    .find_map(|v| v.as_str().filter(|p| !p.is_empty()))
}

/// `:extensions`: pick an extension; its popup opens as if its toolbar button was clicked.
pub fn picker() {
    if !commands::opt_bool("extensions", false) {
        ui::message("extensions: set extensions: true in config.yaml and restart", true);
        return;
    }
    let mut list = runnable();
    list.extend(loaded_unpacked());
    list.sort_by_key(|o| o.0.to_lowercase());
    let rows: Vec<_> = list
        .iter()
        .map(|(name, _, id)| ("setting", name.clone(), id.clone()))
        .collect();
    popup::pick("\u{f0483} Extension popup", &rows, move |i| {
        let (name, folder, id) = &list[i];
        let copied = PathBuf::from(crate::data_dir()).join("ext-store");
        let rid = if folder.starts_with(dir()) || folder.starts_with(copied) { runtime_id(folder, id) } else { id.clone() };
        let manifest = json(folder.join("manifest.json")).unwrap_or_default();
        match popup_page(&manifest) {
            Some(page) => {
                let page = page.trim_start_matches('/');
                // Alloy tabs are invisible to chrome.tabs, so tell the popup which site it
                // was opened on (the cookie exporter reads it; others ignore the parameter).
                let host = tabs::active_url()
                    .split_once("://")
                    .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or("").to_string())
                    .filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:".contains(&b)));
                let query = host.map(|h| format!("{}rembrowser_host={h}", if page.contains('?') { '&' } else { '?' }));
                tabs::extension_popup(&format!("chrome-extension://{rid}/{page}{}", query.unwrap_or_default()));
            }
            None => ui::message(&format!("{name} has no popup or options page (:ext-manage)"), true),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{id_for_path, id_in, popup_page, strip, with_shim_tag, ACTIVE, SHIM};

    #[test]
    fn popup_page_prefers_the_toolbar_button() {
        let m = serde_json::json!({"action": {"default_popup": "ui/popup.html"}, "options_page": "o.html"});
        assert_eq!(popup_page(&m), Some("ui/popup.html"));
        assert_eq!(popup_page(&serde_json::json!({"options_ui": {"page": "o.html"}})), Some("o.html"));
        assert_eq!(popup_page(&serde_json::json!({"action": {}})), None);
    }

    #[test]
    fn patches_apply_once_and_replace_older_ones() {
        // Worker: an old shim at the top goes, the code stays.
        assert_eq!(strip("/*rembrowser:2*/\nold();\n/*end rembrowser*/\ncode();"), "code();");
        assert_eq!(strip("/*rembrowser*/ first_one_line_shim();\ncode();"), "code();");
        assert_eq!(strip("code();"), "code();");
        // Patching twice gives the same file.
        let (worker, script) = (format!("{SHIM}code();"), format!("code();{ACTIVE}"));
        assert_eq!(format!("{SHIM}{}", strip(&worker)), worker);
        assert_eq!(format!("{}{ACTIVE}", strip(&script)), script);
        // Popup page: our script first, added once.
        let html = with_shim_tag("<html><head><script defer src=\"main.js\"></script></head></html>").unwrap();
        assert!(html.find("rembrowser-shim.js").unwrap() < html.find("main.js").unwrap());
        assert_eq!(with_shim_tag(&html), None);
    }

    #[test]
    fn unpacked_id_is_32_letters_a_to_p() {
        let id = id_for_path("/home/u/.local/share/rembrowser/extensions/x");
        assert_eq!(id_in(&id), Some(id.as_str()));
    }

    #[test]
    fn finds_extension_ids() {
        let id = "eimadpbcbfnmbkopoojfekhnkhdbieeh";
        assert_eq!(id_in(id), Some(id));
        assert_eq!(
            id_in("https://chromewebstore.google.com/detail/dark-reader/eimadpbcbfnmbkopoojfekhnkhdbieeh?hl=en"),
            Some(id)
        );
        assert_eq!(id_in("https://example.com/some/page"), None);
        // 32 letters, but not all a–p.
        assert_eq!(id_in("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"), None);
    }
}
