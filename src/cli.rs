//! Terminal subcommands (run before any CEF code):
//!   rembrowser config            open ~/.config/rembrowser/config.yaml in your editor
//!   rembrowser update            rembrowser, Widevine, filter lists (--system: also the
//!                                Chromium engine, through a full system upgrade)
//!   rembrowser uninstall [--purge]
//!   rembrowser help | version

use crate::{adblock, config, widevine};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const HELP: &str = "rembrowser — vim-native, privacy-first Chromium browser

usage:
  rembrowser [URL]                open URL (default: your home page)
  rembrowser config               edit ~/.config/rembrowser/config.yaml (applies live)
  rembrowser update               update rembrowser, Widevine (DRM) and the ad-block lists
  rembrowser update --system      the same, after a full system upgrade (pacman -Syu),
                                  which is what updates the Chromium engine on Arch
  rembrowser uninstall [--purge]  remove rembrowser (--purge: also profile, cache, config)
  rembrowser version";

/// Some(exit code) if argv was a subcommand.
pub fn run() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str)? {
        "config" => {
            config::open_in_editor(true);
            0
        }
        "update" => update(args.iter().any(|a| a == "--system")),
        "uninstall" => uninstall(args.iter().any(|a| a == "--purge")),
        "help" | "--help" | "-h" => {
            println!("{HELP}");
            0
        }
        "version" | "--version" | "-V" => {
            println!(
                "rembrowser {} (Chromium {})",
                env!("CARGO_PKG_VERSION"),
                cef::sys::CHROME_VERSION_MAJOR
            );
            0
        }
        _ => return None,
    };
    Some(code)
}

#[derive(PartialEq)]
enum Install {
    /// pacman package (/usr/bin/rembrowser): engine = Arch's cef.
    Pacman,
    /// packaging/portable bundle in ~/.local/opt/rembrowser: engine bundled.
    Portable(PathBuf),
    /// Running out of a build directory.
    Dev,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn install_kind() -> Install {
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .unwrap_or_default();
    if exe == Path::new("/usr/bin/rembrowser")
        && ok(Command::new("pacman")
            .args(["-Qqo", "/usr/bin/rembrowser"])
            .stdout(std::process::Stdio::null()))
    {
        return Install::Pacman;
    }
    let opt = home().join(".local/opt/rembrowser");
    if exe.parent() == Some(opt.as_path()) {
        return Install::Portable(opt);
    }
    Install::Dev
}

/// The checkout this binary was built from (REMBROWSER_SRC overrides).
fn source_dir() -> Option<PathBuf> {
    let dir = std::env::var("REMBROWSER_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    dir.join("Cargo.toml").exists().then_some(dir)
}

const INSTALLER: &str =
    "https://raw.githubusercontent.com/nganaremba-rem/rembrowser/main/install.sh";

/// Portable bundle: the installer fetches and installs the latest release.
fn reinstall_latest_release() -> bool {
    ok(Command::new("sh").arg("-c").arg(format!(
        "curl -fsSL {INSTALLER} | REMBROWSER_PORTABLE=1 bash"
    )))
}

fn ok(c: &mut Command) -> bool {
    c.status().is_ok_and(|s| s.success())
}

fn step(n: u8, title: &str) {
    println!("\n\x1b[1;35m[{n}/4]\x1b[0m \x1b[1m{title}\x1b[0m");
}

fn confirm(q: &str) -> bool {
    print!("{q} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && matches!(line.trim(), "y" | "Y" | "yes")
}

/// Version of Arch's `cef` package: installed, or what the repos offer as of the last
/// database sync (no network, no root).
fn cef_version(repo: bool) -> Option<String> {
    let out = Command::new("pacman")
        .args([if repo { "-Si" } else { "-Qi" }, "cef"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.starts_with("Version"))?;
    Some(line.split_once(':')?.1.trim().to_string())
}

/// Chromium major of a cef package version ("152.0.6-1", "1:153.0.2-1").
fn major(version: &str) -> Option<i32> {
    version.rsplit(':').next()?.split('.').next()?.parse().ok()
}

/// Step 1 on Arch without --system: the engine is a system package, and rembrowser
/// leaves those to the user. Says what is installed, what is on offer, and whether a
/// newer Chromium will need a rembrowser built for it.
fn engine_note(installed: Option<&str>, repo: Option<&str>, built: i32) -> String {
    let mut out = format!(
        "  cef {} (Arch package) — not touched. It updates with your system:\n  sudo pacman -Syu   or   rembrowser update --system",
        installed.unwrap_or("?")
    );
    if let Some(r) = repo.filter(|r| Some(*r) != installed) {
        out += &format!("\n  cef {r} is available");
        if major(r).is_some_and(|m| m != built) {
            out += &format!(
                " — a new Chromium: this rembrowser is built for {built} and needs its cef crate\n  moved to that version before the system upgrade (pacman would refuse it otherwise)"
            );
        }
    }
    out
}

fn update(system: bool) -> i32 {
    let kind = install_kind();
    let mut failed = false;

    step(1, "Chromium engine");
    match &kind {
        // Arch has no safe single-package upgrade (cef links system libs like ICU), so
        // the engine only moves with a full -Syu — and only when asked for (--system).
        Install::Pacman if !system => println!(
            "{}",
            engine_note(
                cef_version(false).as_deref(),
                cef_version(true).as_deref(),
                cef::sys::CHROME_VERSION_MAJOR
            )
        ),
        Install::Pacman => {
            // A new Chromium *major* needs a rembrowser built for that CEF API first.
            match cef_version(true).as_deref().and_then(major) {
                Some(m) if m != cef::sys::CHROME_VERSION_MAJOR => {
                    println!(
                        "  Chromium {m} is out, but this rembrowser is built for {} — skipping the system upgrade\n  (pacman would refuse it). Update rembrowser's cef crate to {m}, rebuild, then run this again.",
                        cef::sys::CHROME_VERSION_MAJOR
                    );
                }
                _ => failed |= !ok(Command::new("sudo").args(["pacman", "-Syu"])),
            }
        }
        Install::Portable(_) => println!("  bundled — it updates with rembrowser (next step)"),
        Install::Dev => {
            println!("  dev build: uses the system cef (update it with your package manager)")
        }
    }

    step(2, "rembrowser");
    match source_dir() {
        // Portable installs (no checkout): fetch the latest release.
        _ if matches!(kind, Install::Portable(_)) => failed |= !reinstall_latest_release(),
        None => println!(
            "  no source checkout found (set REMBROWSER_SRC) — run the installer again:\n  curl -fsSL {INSTALLER} | bash"
        ),
        Some(src) => {
            if src.join(".git").exists()
                && ok(Command::new("git")
                    .arg("-C")
                    .arg(&src)
                    .args(["rev-parse", "--abbrev-ref", "@{u}"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null()))
            {
                failed |= !ok(Command::new("git")
                    .arg("-C")
                    .arg(&src)
                    .args(["pull", "--ff-only"]));
            }
            let built = match &kind {
                Install::Pacman => ok(Command::new("makepkg")
                    .args(["-sif", "--noconfirm"])
                    .current_dir(src.join("packaging/arch"))),
                Install::Portable(_) => unreachable!(),
                Install::Dev => ok(Command::new("cargo")
                    .args(["build", "--release"])
                    .current_dir(&src)),
            };
            failed |= !built;
        }
    }

    step(3, "Widevine (DRM)");
    match widevine::update() {
        Ok(msg) => println!("  {msg}"),
        Err(e) => {
            println!("  failed: {e}");
            failed = true;
        }
    }

    step(4, "Ad-block filter lists");
    if adblock::update() {
        println!("  refreshed (used from the next start)");
    } else {
        println!("  failed (offline?) — the previous lists stay in use");
        failed = true;
    }

    println!(
        "\n{}",
        if failed {
            "done, with errors above"
        } else {
            "all up to date — restart rembrowser to use the new versions"
        }
    );
    i32::from(failed)
}

fn uninstall(purge: bool) -> i32 {
    let kind = install_kind();
    if kind == Install::Dev {
        println!("rembrowser isn't installed (this is a build directory) — nothing to remove");
        return 1;
    }
    if !confirm("Remove rembrowser?") {
        return 1;
    }
    let removed = match &kind {
        // -s: also cef etc. if nothing else needs them (pacman lists them and asks).
        Install::Pacman => ok(Command::new("sudo").args(["pacman", "-Rns", "rembrowser"])),
        Install::Portable(dir) => {
            let data = std::env::var("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home().join(".local/share"));
            let _ = std::fs::remove_dir_all(dir);
            for f in [
                home().join(".local/bin/rembrowser"),
                data.join("applications/rembrowser.desktop"),
                data.join("icons/hicolor/scalable/apps/rembrowser.svg"),
            ] {
                let _ = std::fs::remove_file(f);
            }
            true
        }
        Install::Dev => unreachable!(),
    };
    if !removed {
        return 1;
    }
    println!("rembrowser removed.");
    let data = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share"));
    let cache = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".cache"));
    let conf = config::path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let dirs = [data.join("rembrowser"), cache.join("rembrowser"), conf];
    if purge {
        println!("\nAlso delete (logins, cookies, history, bookmarks, permissions, settings):");
        dirs.iter().for_each(|d| println!("  {}", d.display()));
        if confirm("Delete these?") {
            dirs.iter().for_each(|d| {
                let _ = std::fs::remove_dir_all(d);
            });
            println!("deleted.");
        }
    } else {
        println!(
            "Kept your profile and settings ({} …) — `rembrowser uninstall --purge` removes them.",
            dirs[0].display()
        );
    }
    0
}

#[cfg(test)]
mod tests {
    use super::{engine_note, major};

    #[test]
    fn engine_step_leaves_system_packages_alone() {
        assert_eq!(major("152.0.6-1"), Some(152));
        assert_eq!(major("1:153.0.2-1"), Some(153));
        // Up to date: nothing on offer.
        let note = engine_note(Some("152.0.6-1"), Some("152.0.6-1"), 152);
        assert!(note.contains("cef 152.0.6-1") && !note.contains("available"));
        // A point release: offered, no rebuild needed.
        let note = engine_note(Some("152.0.6-1"), Some("152.0.9-1"), 152);
        assert!(note.contains("cef 152.0.9-1 is available") && !note.contains("new Chromium"));
        // A new major: rembrowser must be built for it first.
        assert!(engine_note(Some("152.0.6-1"), Some("153.0.2-1"), 152).contains("new Chromium"));
    }
}
