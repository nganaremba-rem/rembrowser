//! Built-in pickers in the popup (no fuzzel / vimb-pick needed): tabs, history,
//! bookmarks, harpoon pins, read-later queue, site permissions, keys. Same data files
//! as vimb-pick (~/.local/share/vimb/{history,bookmark,harpoon,queue}), so both keep
//! working. Rows can be edited / deleted where that makes sense (popup::pick_with).

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

/// A copy of `line`, unless it is entry `drop` (for commands::edit_lines).
fn unless(drop: usize) -> impl FnMut(usize, &str) -> Option<String> {
    move |i, line| (i != drop).then(|| line.to_string())
}

/// Recent pages, newest first, each URL once: (url, title).
fn history_entries() -> Vec<(String, String)> {
    let text = std::fs::read_to_string(commands::data_dir().join("history")).unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    text.lines()
        .rev()
        .filter_map(|l| {
            let (u, t) = l.split_once('\t').unwrap_or((l, ""));
            (!u.is_empty() && seen.insert(u)).then(|| (u.to_string(), t.to_string()))
        })
        // ponytail: cap keeps the payload small; page the file if history grows huge.
        .take(3000)
        .collect()
}

pub fn history() {
    let hist = history_entries();
    let rows: Vec<_> = hist
        .iter()
        .map(|(u, t)| ("history", if t.is_empty() { u.clone() } else { t.clone() }, u.clone()))
        .collect();
    let urls: Vec<String> = hist.into_iter().map(|(u, _)| u).collect();
    let open = urls.clone();
    popup::pick_with(
        "\u{f02da} History",
        &rows,
        &[popup::DELETE],
        move |i| commands::open(&open[i], false),
        move |_, i| {
            // Every visit of that page.
            let url = urls[i].as_str();
            commands::edit_lines(&commands::data_dir().join("history"), |_, l| {
                (l.split('\t').next() != Some(url)).then(|| l.to_string())
            });
            history();
        },
    );
}

pub fn history_clear() {
    popup::confirm("Forget all history?", "Clear", |yes| {
        if yes {
            let _ = std::fs::write(commands::data_dir().join("history"), "");
            ui::message("history cleared", false);
        }
    });
}

pub fn bookmarks() {
    let marks = commands::bookmarks();
    let n = marks.len();
    // Newest first: row i is file entry n-1-i.
    let rows: Vec<_> = marks
        .iter()
        .rev()
        .map(|(u, t, tags)| {
            let name = if t.is_empty() { u } else { t };
            let name = match tags.is_empty() {
                true => name.clone(),
                false => format!("{name}  [{tags}]"),
            };
            ("bookmark", name, u.clone())
        })
        .collect();
    let urls: Vec<String> = rows.iter().map(|r| r.2.clone()).collect();
    popup::pick_with(
        "\u{f00c0} Bookmarks",
        &rows,
        &[popup::EDIT, popup::DELETE],
        move |i| commands::open(&urls[i], false),
        move |act, i| {
            let entry = n - 1 - i;
            if act == "edit" {
                bookmark_edit(entry, &marks[entry]);
            } else {
                commands::edit_lines(&commands::data_dir().join("bookmark"), unless(entry));
                ui::refresh_bookmarks();
                bookmarks();
            }
        },
    );
}

fn bookmark_edit(entry: usize, (url, title, tags): &(String, String, String)) {
    let fields = [("Title", title.as_str()), ("URL", url.as_str()), ("Tags", tags.as_str())];
    popup::form("\u{f00c0} Edit bookmark", &fields, move |v| {
        if let [title, url, tags] = v.as_slice() {
            if !url.trim().is_empty() {
                let line = format!("{}\t{title}\t{tags}", url.trim());
                commands::edit_lines(&commands::data_dir().join("bookmark"), |i, l| {
                    Some(if i == entry { line.clone() } else { l.to_string() })
                });
                ui::refresh_bookmarks();
            }
        }
        bookmarks();
    });
}

/// `:queue`: the read-later list. Opening an entry takes it off the list.
pub fn queue() {
    let path = commands::data_dir().join("queue");
    let urls: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect();
    let rows: Vec<_> = urls.iter().map(|u| ("history", u.clone(), String::new())).collect();
    let del = path.clone();
    popup::pick_with(
        "\u{f02da} Read later",
        &rows,
        &[popup::DELETE],
        move |i| {
            commands::edit_lines(&path, unless(i));
            commands::open(&urls[i], false);
        },
        move |_, i| {
            commands::edit_lines(&del, unless(i));
            queue();
        },
    );
}

/// `:permissions`: what sites may always / never do; delete = ask again next time.
pub fn permissions() {
    let list = crate::permissions::remembered();
    let rows: Vec<_> = list
        .iter()
        .map(|(origin, _, allow, what)| {
            let verdict = if *allow { "always allow" } else { "never allow" };
            ("setting", origin.clone(), format!("{verdict}: {what}"))
        })
        .collect();
    let open: Vec<String> = list.iter().map(|l| l.0.clone()).collect();
    popup::pick_with(
        "\u{f0483} Permissions",
        &rows,
        &[popup::DELETE],
        move |i| commands::open(&open[i], false),
        move |_, i| {
            crate::permissions::forget(&list[i].0, &list[i].1);
            permissions();
        },
    );
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
    let open = pins.clone();
    popup::pick_with(
        "\u{f0403} Harpoon",
        &rows,
        &[popup::EDIT, popup::DELETE],
        move |i| commands::open(&open[i].0, false),
        move |act, i| {
            let mut pins = pins.clone();
            if act == "edit" {
                let (url, title) = pins[i].clone();
                popup::form("\u{f0403} Edit pin", &[("Title", &title), ("URL", &url)], move |v| {
                    if let [title, url] = v.as_slice() {
                        if !url.trim().is_empty() {
                            pins[i] = (url.trim().to_string(), title.clone());
                            pins_write(&pins);
                        }
                    }
                    self::pins();
                });
            } else {
                pins.remove(i);
                pins_write(&pins);
                self::pins();
            }
        },
    );
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
