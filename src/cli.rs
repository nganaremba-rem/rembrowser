//! Terminal subcommands (run before any CEF code):
//!   rembrowser config            open ~/.config/rembrowser/config.yaml in your editor
//!   rembrowser update            engine + system packages, rembrowser, Widevine, filter lists
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
  rembrowser update               update Chromium engine + system packages, rembrowser,
                                  Widevine (DRM) and the ad-block lists
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
        "update" => update(),
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

fn update() -> i32 {
    let kind = install_kind();
    let mut failed = false;

    step(1, "Chromium engine + system packages");
    match &kind {
        Install::Pacman => {
            // Arch: never a partial upgrade (cef links system libs like ICU), so -Syu.
            // A new Chromium *major* needs a rembrowser built for that CEF API first.
            let repo = Command::new("pacman")
                .args(["-Si", "cef"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
            let major = repo
                .as_deref()
                .and_then(|t| t.lines().find(|l| l.starts_with("Version")))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|v| v.trim().split('.').next()?.parse::<i32>().ok());
            match major {
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
        None => println!(
            "  no source checkout found (set REMBROWSER_SRC) — download a newer release instead"
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
                Install::Portable(_) => {
                    ok(Command::new(src.join("packaging/portable/build.sh")).current_dir(&src)) && {
                        let ver = env!("CARGO_PKG_VERSION");
                        ok(&mut Command::new(src.join(format!(
                            "dist/rembrowser-{ver}-x86_64-linux/install.sh"
                        ))))
                    }
                }
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
        println!("\nAlso delete (logins, cookies, permissions, settings):");
        dirs.iter().for_each(|d| println!("  {}", d.display()));
        if confirm("Delete these?") {
            dirs.iter().for_each(|d| {
                let _ = std::fs::remove_dir_all(d);
            });
            println!("deleted. (vimb's shared history/bookmarks in ~/.local/share/vimb are kept)");
        }
    } else {
        println!(
            "Kept your profile and settings ({} …) — `rembrowser uninstall --purge` removes them.",
            dirs[0].display()
        );
    }
    0
}
