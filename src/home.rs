//! The homescreen (new tabs, `gh`, start page): home.html served from inside the
//! browser at a fake https origin. `.invalid` never resolves, and this handler
//! answers before any network lookup — so nothing leaves the machine, yet the page
//! is a normal secure origin with its own localStorage (that's where tiles live).

use cef::*;
use std::sync::{Arc, Mutex};

pub const URL: &str = "https://home.rembrowser.invalid/";
const HTML: &str = include_str!("home.html");

pub fn is_home(url: &str) -> bool {
    url.starts_with(URL)
}

/// `set home-page=home` (or unset) = this page; anything else is a normal URL.
pub fn home_page() -> String {
    match crate::commands::opt("home-page").as_deref() {
        None | Some("") | Some("home") | Some("rem:home") => URL.into(),
        Some(u) => u.into(),
    }
}

pub fn handler() -> ResourceHandler {
    HomeHandler::new(Arc::new(Mutex::new(0)))
}

wrap_resource_handler! {
    struct HomeHandler {
        offset: Arc<Mutex<usize>>,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut ::std::os::raw::c_int>,
            _callback: Option<&mut Callback>,
        ) -> ::std::os::raw::c_int {
            if let Some(h) = handle_request {
                *h = 1;
            }
            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            if let Some(r) = response {
                r.set_status(200);
                r.set_mime_type(Some(&"text/html".into()));
                r.set_charset(Some(&"utf-8".into()));
            }
            if let Some(len) = response_length {
                *len = HTML.len() as i64;
            }
        }

        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: ::std::os::raw::c_int,
            bytes_read: Option<&mut ::std::os::raw::c_int>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> ::std::os::raw::c_int {
            let Ok(mut off) = self.offset.lock() else { return 0 };
            let rest = &HTML.as_bytes()[*off..];
            let n = rest.len().min(bytes_to_read.max(0) as usize);
            if n == 0 || data_out.is_null() {
                return 0;
            }
            // SAFETY: CEF hands a buffer of at least bytes_to_read bytes.
            unsafe { std::ptr::copy_nonoverlapping(rest.as_ptr(), data_out, n) };
            *off += n;
            if let Some(b) = bytes_read {
                *b = n as i32;
            }
            1
        }
    }
}
