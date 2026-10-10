//! Widevine (DRM: Netflix, Prime, Spotify, ...) without Google Chrome installed.
//!
//! Chromium loads a CDM named by a hint file at startup. Use an installed one if
//! there is one (Chrome keeps its copy fresh); otherwise fetch it once from Google's
//! component-update service — the same signed CRX Chrome downloads — verify its
//! SHA-256, unpack it to ~/.local/share/rembrowser/WidevineCdm, and enable it on the
//! next start.

use crate::{commands, ui};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

const APP_ID: &str = "oimompecagnajdejgnnjijobebaeigek";

fn own_dir() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map_or_else(
            |_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share"),
            PathBuf::from,
        )
        .join("rembrowser/WidevineCdm")
}

/// An installed CDM directory (has manifest.json + the linux_x64 library).
fn find() -> Option<PathBuf> {
    [
        PathBuf::from("/opt/google/chrome/WidevineCdm"),
        PathBuf::from("/usr/lib/chromium/WidevineCdm"),
        own_dir(),
    ]
    .into_iter()
    .find(|d| {
        d.join("manifest.json").exists()
            && d.join("_platform_specific/linux_x64/libwidevinecdm.so")
                .exists()
    })
}

/// Point Chromium at a CDM (`profile` = root cache path), or start fetching one.
pub fn setup(profile: &str) {
    let Some(cdm) = find() else {
        std::thread::spawn(|| match fetch() {
            Ok(v) => ui::run_on_ui(0, move || {
                ui::toast(
                    &format!("✓  DRM (Widevine {v}) installed — restart to enable"),
                    false,
                )
            }),
            Err(e) => eprintln!("rembrowser: Widevine download failed: {e}"),
        });
        return;
    };
    let dir = format!("{profile}/WidevineCdm");
    let hint = format!(r#"{{"Path":"{}"}}"#, cdm.display());
    let path = format!("{dir}/latest-component-updated-widevine-cdm");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(hint.as_str()) {
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = std::fs::write(&path, hint) {
            eprintln!("rembrowser: widevine hint: {e}");
        }
    }
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = commands::command("curl")
        // No total timeout (22 MB on a slow link is fine in the background): give up
        // only when stalled under 1 kB/s for a minute.
        .args([
            "-fsSL",
            "--connect-timeout",
            "20",
            "--speed-limit",
            "1000",
            "--speed-time",
            "60",
        ])
        .args(args)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("curl exit {}", out.status));
    }
    Ok(out.stdout)
}

struct Latest {
    name: String,
    sha: String,
    version: String,
    bases: Vec<String>,
}

/// What the update service currently offers.
fn latest() -> Result<Latest, String> {
    let req = format!(
        r#"{{"request":{{"@os":"linux","@updater":"chromium","acceptformat":"crx3","protocol":"3.1","prodversion":"152.0.0.0","updaterversion":"152.0.0.0","arch":"x64","os":{{"platform":"Linux","arch":"x86_64","version":"6.0"}},"app":[{{"appid":"{APP_ID}","version":"0.0.0.0","updatecheck":{{}}}}]}}}}"#
    );
    let body = curl(&[
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
        "-d",
        &req,
        "https://update.googleapis.com/service/update2/json",
    ])?;
    // The JSON is prefixed with ")]}'" (XSSI guard).
    let text = String::from_utf8_lossy(&body);
    let json: serde_json::Value = serde_json::from_str(text.trim_start_matches(")]}'").trim())
        .map_err(|e| format!("update reply: {e}"))?;
    let check = &json["response"]["app"][0]["updatecheck"];
    let pkg = &check["manifest"]["packages"]["package"][0];
    let (Some(name), Some(sha), Some(version)) = (
        pkg["name"].as_str(),
        pkg["hash_sha256"].as_str(),
        check["manifest"]["version"].as_str(),
    ) else {
        return Err(format!(
            "no Widevine in update reply (status {})",
            check["status"]
        ));
    };
    let bases = check["urls"]["url"]
        .as_array()
        .map(|u| {
            u.iter()
                .filter_map(|x| x["codebase"].as_str())
                .filter(|c| c.starts_with("https://"))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    Ok(Latest {
        name: name.into(),
        sha: sha.into(),
        version: version.into(),
        bases,
    })
}

/// `rembrowser update`: refresh our own copy (a Chrome/Chromium copy updates with that browser).
pub fn update() -> Result<String, String> {
    if let Some(d) = find().filter(|d| *d != own_dir()) {
        return Ok(format!(
            "using {} (updated together with that browser)",
            d.display()
        ));
    }
    let have = std::fs::read_to_string(own_dir().join("manifest.json"))
        .ok()
        .and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok())
        .and_then(|m| m["version"].as_str().map(String::from));
    let l = latest()?;
    if have.as_deref() == Some(l.version.as_str()) {
        return Ok(format!("{} (up to date)", l.version));
    }
    fetch_from(l).map(|v| format!("{v} installed — restart rembrowser to use it"))
}

/// Download, verify, unpack. Returns the CDM version.
fn fetch() -> Result<String, String> {
    fetch_from(latest()?)
}

fn fetch_from(l: Latest) -> Result<String, String> {
    let Latest {
        name,
        sha,
        version,
        bases,
    } = l;
    // Resumable: a stalled or interrupted download continues from the .part file
    // (next mirror, or next browser start).
    let part = own_dir().with_extension("crx3.part");
    let _ = std::fs::create_dir_all(part.parent().ok_or("bad data dir")?);
    let part_s = part.to_string_lossy().into_owned();
    let mut last = String::from("no https download URL");
    let done = bases.iter().any(|base| {
        match curl(&["-C", "-", "-o", &part_s, &format!("{base}{name}")]) {
            Ok(_) => true,
            Err(e) => {
                last = e;
                false
            }
        }
    });
    if !done {
        return Err(last);
    }
    let crx = std::fs::read(&part).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&part);
    let got = Sha256::digest(&crx)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if got != sha {
        return Err(format!("sha256 mismatch: {got} != {sha}"));
    }
    let tmp = own_dir().with_extension("partial");
    let _ = std::fs::remove_dir_all(&tmp);
    unzip(crx3_zip(&crx)?, &tmp)?;
    let dest = own_dir();
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    Ok(version)
}

/// CRX3 = "Cr24", u32 version, u32 header length, header (signatures), zip.
fn crx3_zip(crx: &[u8]) -> Result<&[u8], String> {
    if crx.len() < 12 || &crx[..4] != b"Cr24" {
        return Err("not a CRX file".into());
    }
    let header = u32::from_le_bytes(crx[8..12].try_into().unwrap()) as usize;
    crx.get(12 + header..).ok_or_else(|| "truncated CRX".into())
}

fn u16_at(b: &[u8], i: usize) -> usize {
    b.get(i..i + 2)
        .map_or(0, |x| u16::from_le_bytes([x[0], x[1]]) as usize)
}

fn u32_at(b: &[u8], i: usize) -> usize {
    b.get(i..i + 4)
        .map_or(0, |x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as usize)
}

/// Minimal zip reader (central directory; stored + deflate) into `dest`.
fn unzip(zip: &[u8], dest: &Path) -> Result<(), String> {
    let eocd = (0..zip.len().saturating_sub(21))
        .rev()
        .find(|&i| zip[i..].starts_with(&[0x50, 0x4b, 0x05, 0x06]))
        .ok_or("no zip directory")?;
    let (count, mut p) = (u16_at(zip, eocd + 10), u32_at(zip, eocd + 16));
    for _ in 0..count {
        if !zip
            .get(p..)
            .is_some_and(|b| b.starts_with(&[0x50, 0x4b, 0x01, 0x02]))
        {
            return Err("bad zip entry".into());
        }
        let (method, csize, size) = (
            u16_at(zip, p + 10),
            u32_at(zip, p + 20),
            u32_at(zip, p + 24),
        );
        let (nlen, xlen, clen, local) = (
            u16_at(zip, p + 28),
            u16_at(zip, p + 30),
            u16_at(zip, p + 32),
            u32_at(zip, p + 42),
        );
        let name =
            String::from_utf8_lossy(zip.get(p + 46..p + 46 + nlen).ok_or("bad name")?).into_owned();
        p += 46 + nlen + xlen + clen;
        // Never write outside dest.
        if name.starts_with('/') || name.split('/').any(|c| c == "..") {
            return Err(format!("unsafe path in zip: {name}"));
        }
        let out = dest.join(&name);
        if name.ends_with('/') {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        let start = local + 30 + u16_at(zip, local + 26) + u16_at(zip, local + 28);
        let data = zip.get(start..start + csize).ok_or("truncated zip data")?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => {
                let mut v = Vec::with_capacity(size);
                flate2::read::DeflateDecoder::new(data)
                    .read_to_end(&mut v)
                    .map_err(|e| e.to_string())?;
                v
            }
            m => return Err(format!("zip method {m} unsupported")),
        };
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Network: `cargo test -- --ignored widevine_download` (needs XDG_DATA_HOME set).
    #[test]
    #[ignore]
    fn widevine_download() {
        let v = fetch().expect("fetch");
        assert!(
            own_dir()
                .join("_platform_specific/linux_x64/libwidevinecdm.so")
                .exists(),
            "{v}"
        );
    }

    #[test]
    fn rejects_non_crx_and_bad_zip() {
        assert!(crx3_zip(b"PK\x03\x04").is_err());
        assert!(unzip(b"not a zip at all, nothing here", Path::new("/nonexistent")).is_err());
    }
}
