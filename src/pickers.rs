//! Built-in pickers in the popup (no fuzzel / vimb-pick needed): tabs, history,
//! bookmarks, harpoon pins, keys. Same data files as vimb-pick
//! (~/.local/share/vimb/{history,bookmark,harpoon}), so both keep working.

use crate::{commands, popup, tabs, ui, vim};

pub fn tabs() {
    let rows: Vec<_> = tabs::titles()
        .into_iter()
        .enumerate()
        .map(|(i, (t, u))| {
            (
                "tab",
                format!("{}  {}", i + 1, if t.is_empty() { &u } else { &t }),
                u,
            )
        })
        .collect();
    popup::pick("\u{f04e9} Tabs", &rows, tabs::select);
}

fn open_rows(title: &str, kind: &str) {
    let rows: Vec<_> = popup::urls()
        .into_iter()
        .filter(|(k, ..)| *k == kind)
        .map(|(k, t, u)| (k, if t.is_empty() { u.clone() } else { t }, u))
        .collect();
    let urls: Vec<String> = rows.iter().map(|r| r.2.clone()).collect();
    popup::pick(title, &rows, move |i| commands::open(&urls[i], false));
}

pub fn history() {
    open_rows("\u{f02da} History", "history");
}

pub fn bookmarks() {
    open_rows("\u{f00c0} Bookmarks", "bookmark");
}

// ── harpoon: numbered pins, one "url\ttitle" per line ─────────────

fn pins_read() -> Vec<(String, String)> {
    parse_pins(&std::fs::read_to_string(commands::data_dir().join("harpoon")).unwrap_or_default())
}

fn parse_pins(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (u, t) = l.split_once('\t').unwrap_or((l, ""));
            (u.to_string(), t.to_string())
        })
        .collect()
}

fn pins_write(pins: &[(String, String)]) {
    let dir = commands::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let text: String = pins.iter().map(|(u, t)| format!("{u}\t{t}\n")).collect();
    let _ = std::fs::write(dir.join("harpoon"), text);
}

pub fn pins() {
    let pins = pins_read();
    let rows: Vec<_> = pins
        .iter()
        .enumerate()
        .map(|(i, (u, t))| {
            (
                "pin",
                format!("{}  {}", i + 1, if t.is_empty() { u } else { t }),
                u.clone(),
            )
        })
        .collect();
    popup::pick("\u{f0403} Harpoon", &rows, move |i| {
        commands::open(&pins[i].0, false)
    });
}

/// `:pin add` appends the page (moving it if pinned), `:pin N` sets slot N.
pub fn pin(slot: &str) {
    let (url, title) = (tabs::active_url(), tabs::active_title());
    if url.is_empty() {
        return;
    }
    let mut pins = pins_read();
    let n = if slot == "add" || slot.is_empty() {
        pins.retain(|(u, _)| *u != url);
        pins.push((url, title.clone()));
        pins.len()
    } else {
        let Ok(n) = slot.parse::<usize>() else {
            ui::message("usage: :pin add | :pin N", true);
            return;
        };
        if n == 0 {
            return;
        }
        // Pad gaps so slot numbers stay stable (vimb-pick does the same).
        while pins.len() < n {
            pins.push(("about:blank".into(), "(empty)".into()));
        }
        pins[n - 1] = (url, title.clone());
        n
    };
    pins_write(&pins);
    ui::message(&format!("harpoon {n}: {title}"), false);
}

pub fn jump(n: &str) {
    let pins = pins_read();
    match n
        .parse::<usize>()
        .ok()
        .and_then(|n| pins.get(n.wrapping_sub(1)))
    {
        Some((u, _)) => commands::open(u, false),
        None => ui::message(&format!("harpoon: slot {n} empty"), true),
    }
}

pub fn keys() {
    let sheet = vim::cheatsheet();
    let rows: Vec<_> = sheet
        .iter()
        .map(|(k, d)| ("key", d.clone(), k.clone()))
        .collect();
    popup::pick("\u{f11c} Keys", &rows, move |i| {
        vim::run_keys(&sheet[i].0, false)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_harpoon_file() {
        let pins = parse_pins("https://a.com/\tA\n\nhttps://b.com/\n");
        assert_eq!(
            pins,
            vec![
                ("https://a.com/".into(), "A".into()),
                ("https://b.com/".into(), String::new())
            ]
        );
    }
}
