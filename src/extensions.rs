//! Optional Chrome extensions (`extensions: true`).
//!
//! The Web Store only installs into Chrome ("Switch to Chrome"), so `:ext-install`
//! fetches the extension's package from Google's download service itself — the file
//! Chrome would get — and unpacks it to ~/.local/share/rembrowser/extensions/<id>.
//! Every folder there is handed to Chromium with --load-extension at startup, so
//! installs and removals apply after a restart. `:extensions` (chrome://extensions)
//! shows them with their options and on/off switches; `:ext-popup` opens the page
//! behind an extension's toolbar button in its own window.
//!
//! ponytail: loaded unpacked, an extension without a "key" in its manifest gets its
//! id from its folder path, not from the store (`runtime_id`). Write the package's
//! public key into manifest.json if one ever needs its store id. No auto-update
//! either: `:ext-install` again fetches the latest.

use crate::{commands, popup, tabs, ui, widevine};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn dir() -> PathBuf {
    PathBuf::from(crate::data_dir()).join("extensions")
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

/// Value for --load-extension: the installed folders (None = nothing installed).
pub fn load_switch() -> Option<String> {
    let dirs: Vec<String> = installed()
        .iter()
        .map(|(id, _)| dir().join(id).display().to_string())
        .collect();
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
fn runtime_id(id: &str) -> String {
    let folder = dir().join(id);
    if json(folder.join("manifest.json")).is_some_and(|m| m["key"].is_string()) {
        return id.into();
    }
    let real = folder.canonicalize().unwrap_or(folder);
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

/// `:ext-popup`: pick an extension; its popup opens in a Chromium window.
pub fn popup_picker() {
    if !commands::opt_bool("extensions", false) {
        ui::message("ext-popup: set extensions: true and restart", true);
        return;
    }
    let list = installed();
    let rows: Vec<_> = list
        .iter()
        .map(|(id, name)| ("setting", name.clone(), id.clone()))
        .collect();
    popup::pick("\u{f0483} Extension popup", &rows, move |i| {
        let (id, name) = &list[i];
        let manifest = json(dir().join(id).join("manifest.json")).unwrap_or_default();
        match popup_page(&manifest) {
            Some(page) => {
                let page = page.trim_start_matches('/');
                tabs::chrome_window(&format!("chrome-extension://{}/{page}", runtime_id(id)), false);
            }
            None => ui::message(&format!("{name} has no popup or options page"), true),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{id_for_path, id_in, popup_page};

    #[test]
    fn popup_page_prefers_the_toolbar_button() {
        let m = serde_json::json!({"action": {"default_popup": "ui/popup.html"}, "options_page": "o.html"});
        assert_eq!(popup_page(&m), Some("ui/popup.html"));
        assert_eq!(popup_page(&serde_json::json!({"options_ui": {"page": "o.html"}})), Some("o.html"));
        assert_eq!(popup_page(&serde_json::json!({"action": {}})), None);
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
