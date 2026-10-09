//! Debloat + privacy, all at runtime (no Chromium rebuild): command-line switches,
//! disabled features, profile prefs and the Widevine hint file.
//! Verify changes with spike/net.sh (phone-home hosts) and spike/mem.sh (RAM per process).

use cef::*;

/// Boolean switches for the browser process.
const SWITCHES: &[&str] = &[
    // Distro CEF shows Chromium's first-run Terms dialog and blocks the window.
    "no-first-run",
    "no-default-browser-check",
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
    set("host-resolver-rules", &format!("MAP *.{BLACKHOLE} ~NOTFOUND"));
    // Merge with any --disable-features the user passed on the command line.
    let key: CefString = "disable-features".into();
    let user = CefString::from(&cl.switch_value(Some(&key))).to_string();
    let mut features = DISABLED_FEATURES.join(",");
    if !user.is_empty() {
        features = format!("{user},{features}");
    }
    set("disable-features", &features);
    for s in SWITCHES {
        cl.append_switch(Some(&(*s).into()));
    }
    // vimb `set dark-mode=on`: sites see prefers-color-scheme: dark.
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
    ("url_keyed_anonymized_data_collection.enabled", Pref::Bool(false)),
    ("privacy_sandbox.m1.topics_enabled", Pref::Bool(false)),
    ("privacy_sandbox.m1.fledge_enabled", Pref::Bool(false)),
    ("privacy_sandbox.m1.ad_measurement_enabled", Pref::Bool(false)),
    // Block third-party cookies (1 = kBlockThirdParty).
    ("profile.cookie_controls_mode", Pref::Int(1)),
    // WebRTC must not leak the LAN/VPN-bypassing IP.
    ("webrtc.ip_handling_policy", Pref::Str("default_public_interface_only")),
    // vimb: geolocation=never (2 = block). Notifications stay "ask" (default).
    ("profile.default_content_setting_values.geolocation", Pref::Int(2)),
];

/// Must run on the UI thread after the context is initialized.
pub fn apply_prefs() {
    let Some(ctx) = request_context_get_global_context() else { return };
    for (name, pref) in PREFS {
        let Some(mut v) = value_create() else { continue };
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
}

/// Chromium on Linux loads a component-updated CDM from this hint file at startup.
/// Point it at an installed Widevine (Chrome's is newest; Chromium's as fallback).
pub fn write_widevine_hint(profile: &str) {
    const CDMS: &[&str] = &["/opt/google/chrome/WidevineCdm", "/usr/lib/chromium/WidevineCdm"];
    let Some(cdm) = CDMS.iter().find(|p| std::path::Path::new(p).join("manifest.json").exists())
    else {
        eprintln!("rembrowser: no Widevine CDM found (install google-chrome); DRM sites won't play");
        return;
    };
    let dir = format!("{profile}/WidevineCdm");
    let hint = format!(r#"{{"Path":"{cdm}"}}"#);
    let path = format!("{dir}/latest-component-updated-widevine-cdm");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(hint.as_str()) {
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = std::fs::write(&path, hint) {
            eprintln!("rembrowser: widevine hint: {e}");
        }
    }
}
