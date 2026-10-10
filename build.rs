// Bake libcef's location into the binary (RUNPATH), so `rembrowser` starts without
// LD_LIBRARY_PATH. Default: Arch's `cef` package. Portable bundles set
// REMBROWSER_CEF_RPATH='$ORIGIN/lib' (libcef shipped next to the binary).
fn main() {
    let dir = std::env::var("REMBROWSER_CEF_RPATH").unwrap_or_else(|_| "/usr/lib/cef".into());
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{dir}");
    println!("cargo:rerun-if-env-changed=REMBROWSER_CEF_RPATH");
}
