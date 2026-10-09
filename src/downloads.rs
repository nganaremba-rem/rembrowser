//! Downloads: Alloy refuses every download unless a handler says yes.
//! Files go to ~/Downloads (vimb download-path) and never overwrite.

use cef::*;

wrap_download_handler! {
    pub struct RemDownloadHandler;

    impl DownloadHandler {
        // Alloy refuses every download unless this says yes.
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> i32 {
            1
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            _download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> i32 {
            let Some(cb) = callback else { return 0 };
            let name = suggested_name.map(|n| n.to_string()).unwrap_or_default();
            let path = download_path(&name);
            cb.cont(Some(&path.to_string_lossy().as_ref().into()), 0);
            1
        }
    }
}

/// ~/Downloads/<name>, never overwriting: "file (1).pdf", "file (2).pdf", ...
fn download_path(name: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Downloads");
    let _ = std::fs::create_dir_all(&dir);
    // Strip path separators a hostile server could put in the suggested name.
    let name = match name.rsplit(['/', '\\']).next().filter(|n| !n.is_empty() && *n != "..") {
        Some(n) => n,
        None => "download",
    };
    let p = std::path::Path::new(name);
    let (stem, ext) = (
        p.file_stem().map_or(name.into(), |s| s.to_string_lossy()),
        p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default(),
    );
    let mut path = dir.join(name);
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::download_path;

    #[test]
    fn download_names_are_sanitized() {
        let p = download_path("../../etc/passwd");
        assert!(p.ends_with("Downloads/passwd") || p.file_name().unwrap().to_string_lossy().starts_with("passwd ("));
        assert!(download_path("..").ends_with("Downloads/download") || download_path("..").to_string_lossy().contains("download ("));
    }
}

