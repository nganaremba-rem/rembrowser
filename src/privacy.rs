//! Debloat + privacy, all at runtime (no Chromium rebuild): command-line switches,
//! disabled features, profile prefs and the Widevine hint file.
//! Verify changes with spike/net.sh (phone-home hosts) and spike/mem.sh (RAM per process).

use cef::*;

/// Boolean switches for the browser process.
const SWITCHES: &[&str] = &[
    // Distro CEF shows Chromium's first-run Terms dialog and blocks the window.
    "no-first-run",
    "no-default-browser-check",
    // Shader cache on disk: rebuilt cheaply, grows without a bound.
    "disable-gpu-shader-disk-cache",
    // Variations seed, component updater, CRLSet, etc. Widevine still loads via the hint file.
    "disable-background-networking",
    "disable-component-update",
    "disable-sync",
    "disable-domain-reliability",
    "disable-client-side-phishing-detection",
    "disable-default-apps",
    // <a ping> hyperlink auditing.
    "no-pings",
];

/// Features nobody using a vim browser needs. Unknown names are ignored by Chromium,
/// so each entry was verified by behaviour, not by name.
const DISABLED_FEATURES: &[&str] = &[
    // Hidden, preloaded omnibox popup WebUIs: 2 renderers (~90 MB) for an omnibox we don't have.
    "WebUIOmniboxPopup",
    "WebUIOmniboxAimPopup",
    // Optimization Guide: hint fetches + on-device model downloads (multi-GB Gemini Nano).
    "OptimizationHints",
    "OptimizationHintsFetching",
    "OptimizationGuideModelDownloading",
    "OptimizationTargetPrediction",
    "OptimizationGuideOnDeviceModel",
    // Cast/DIAL: LAN discovery traffic (mDNS/SSDP).
    "MediaRouter",
    "DialMediaRouteProvider",
    "CastMediaRouteProvider",
    "GlobalMediaControls",
    // Google services.
    "Translate",
    "AutofillServerCommunication",
    "NetworkTimeServiceQuerying",
    // Privacy Sandbox ad APIs.
    "BrowsingTopics",
    "Fledge",
    "InterestGroupStorage",
    "AdInterestGroupAPI",
    "SharedStorageAPI",
    "PrivateAggregationApi",
    // Memory: speculative prerendered pages (never used with Alloy-style tabs anyway) and
    // a spare renderer started ahead of time (+7 MB, no faster new tab: measured 2026-11).
    // BackForwardCache stays ON: instant back/forward for +3% RAM (8 tabs x 3 pages).
    "Prerender2",
    "SpareRendererForSitePerProcess",
    // Gemini / AI UI.
    "Glic",
    "LensOverlay",
    "HistoryEmbeddings",
    "Compose",
];

/// Chrome's own Google endpoints (sign-in account polling = accounts.google.com/ListAccounts,
/// AI Mode eligibility = google.com/async/folae) are hard-wired at startup. Point them at a
/// reserved .invalid host that resolves to NOTFOUND locally: zero packets leave.
/// Pages you open (google.com, YouTube login, ...) are unaffected.
const BLACKHOLE: &str = "rembrowser.invalid";

pub fn apply_switches(cl: &mut CommandLine) {
    let set = |k: &str, v: &str| cl.append_switch_with_value(Some(&k.into()), Some(&v.into()));
    set("gaia-url", &format!("https://gaia.{BLACKHOLE}"));
    set("google-base-url", &format!("https://google.{BLACKHOLE}"));
    set(
        "host-resolver-rules",
        &format!("MAP *.{BLACKHOLE} ~NOTFOUND"),
    );
    // Native Wayland when there is one (some CEF builds default to X11/XWayland;
    // ozone-platform-hint=auto needs XDG_SESSION_TYPE, which isn't always set).
    if cl.has_switch(Some(&"ozone-platform".into())) == 0 {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|w| !w.is_empty());
        set("ozone-platform", if wayland { "wayland" } else { "x11" });
    }
    // Merge with any --disable-features the user passed on the command line.
    let key: CefString = "disable-features".into();
    let user = CefString::from(&cl.switch_value(Some(&key))).to_string();
    // REMBROWSER_KEEP_FEATURES=A,B leaves those on: to measure what one costs.
    let keep = std::env::var("REMBROWSER_KEEP_FEATURES").unwrap_or_default();
    let mut features = DISABLED_FEATURES
        .iter()
        .filter(|f| !keep.split(',').any(|k| k == **f))
        .copied()
        .collect::<Vec<_>>()
        .join(",");
    if !user.is_empty() {
        features = format!("{user},{features}");
    }
    set("disable-features", &features);
    for s in SWITCHES {
        cl.append_switch(Some(&(*s).into()));
    }
    // Disk cache was unbounded (1.2 GB seen); 300 MB is plenty for one user.
    if cl.has_switch(Some(&"disk-cache-size".into())) == 0 {
        set("disk-cache-size", "314572800");
    }
    // Chrome extensions are opt-in (`extensions: true`): off, none is loaded. On,
    // Chromium gets the folders `:ext-install` unpacked (plus any the user passed).
    if !crate::commands::opt_bool("extensions", false) {
        cl.append_switch(Some(&"disable-extensions".into()));
    } else if let Some(ours) = crate::extensions::load_switch() {
        let key: CefString = "load-extension".into();
        let user = CefString::from(&cl.switch_value(Some(&key))).to_string();
        let dirs = if user.is_empty() { ours } else { format!("{user},{ours}") };
        set("load-extension", &dirs);
    }
    // process-per-site: all tabs of one site share one renderer. 30 same-site tabs:
    // 577 -> 331 MB. The price: those tabs share one main thread and crash together.
    if crate::commands::opt_bool("process-per-site", false) {
        cl.append_switch(Some(&"process-per-site".into()));
    }
    // dark-mode: sites see prefers-color-scheme: dark.
    if crate::commands::opt_bool("dark-mode", false) {
        cl.append_switch(Some(&"force-dark-mode".into()));
    }
}

enum Pref {
    Bool(bool),
    Int(i32),
    Str(&'static str),
}

/// Profile prefs = the Chrome policies we'd otherwise need /etc/chromium/policies for
/// (that dir is shared with the system Chromium). Persisted in the profile, so from
/// the 2nd launch on they are active before any service starts.
const PREFS: &[(&str, Pref)] = &[
    ("signin.allowed", Pref::Bool(false)),
    // No default search engine: no google.com preconnect. Search goes through
    // rembrowser's own shortcuts (dd/g/aw/...).
    ("default_search_provider.enabled", Pref::Bool(false)),
    ("omnibox.ai_mode_settings", Pref::Int(1)),
    ("search.suggest_enabled", Pref::Bool(false)),
    ("alternate_error_pages.enabled", Pref::Bool(false)),
    (
        "url_keyed_anonymized_data_collection.enabled",
        Pref::Bool(false),
    ),
    ("privacy_sandbox.m1.topics_enabled", Pref::Bool(false)),
    ("privacy_sandbox.m1.fledge_enabled", Pref::Bool(false)),
    (
        "privacy_sandbox.m1.ad_measurement_enabled",
        Pref::Bool(false),
    ),
    // Block third-party cookies (1 = kBlockThirdParty).
    ("profile.cookie_controls_mode", Pref::Int(1)),
    // WebRTC must not leak the LAN/VPN-bypassing IP.
    (
        "webrtc.ip_handling_policy",
        Pref::Str("default_public_interface_only"),
    ),
    // geolocation=never (2 = block). Notifications stay "ask" (default).
    (
        "profile.default_content_setting_values.geolocation",
        Pref::Int(2),
    ),
];

/// Must run on the UI thread after the context is initialized.
pub fn apply_prefs() {
    let Some(ctx) = request_context_get_global_context() else {
        return;
    };
    for (name, pref) in PREFS {
        let Some(mut v) = value_create() else {
            continue;
        };
        match pref {
            Pref::Bool(b) => v.set_bool(*b as i32),
            Pref::Int(i) => v.set_int(*i),
            Pref::Str(s) => v.set_string(Some(&(*s).into())),
        };
        // Not CefString::default(): that is a null pointer, and CEF rejects a null
        // `error` out-param with a silent `false`.
        let mut err = CefString::from("");
        if ctx.set_preference(Some(&(*name).into()), Some(&mut v), Some(&mut err)) == 0 {
            eprintln!("rembrowser: pref {name}: {err}");
        }
    }
    allow_payment_cookies(&ctx);
}

/// Third-party cookies are blocked, but card checkouts and 3-D Secure frames often need
/// their own provider's cookie from inside the shop's page. These hosts (and any in the
/// config's `cookie-allow`, space separated) may set cookies as a frame; nothing else.
/// ponytail: the default list is from memory of common providers, not a measured
/// breakage list; add hosts in config when a checkout loops.
const PAYMENT_HOSTS: &[&str] = &[
    "js.stripe.com", "hooks.stripe.com", "m.stripe.network", "checkout.stripe.com",
    "api.razorpay.com", "checkout.razorpay.com",
    "www.paypal.com", "www.paypalobjects.com",
    "pay.google.com", "authentication.cardinalcommerce.com", "centinelapi.cardinalcommerce.com",
];

fn allow_payment_cookies(ctx: &RequestContext) {
    let extra = crate::commands::opt("cookie-allow").unwrap_or_default();
    let hosts = PAYMENT_HOSTS.iter().copied().chain(extra.split_whitespace());
    for host in hosts {
        // Only plain host names: this goes into a URL pattern.
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
            continue;
        }
        let scheme = if host.starts_with("127.") || host == "localhost" { "http" } else { "https" };
        ctx.set_content_setting(
            Some(&format!("{scheme}://{host}/").as_str().into()),
            Some(&"".into()),
            ContentSettingTypes::COOKIES,
            ContentSettingValues::ALLOW,
        );
    }
}
