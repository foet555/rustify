// On the GNU target the WebView2 bindings link against WebView2Loader.dll
// instead of the static loader, so the DLL has to sit next to the exe or the
// program dies at startup with a missing-library error.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let dll = std::path::Path::new("vendor/webview2/WebView2Loader.dll");
    if !dll.exists() {
        return;
    }
    let Ok(out) = std::env::var("OUT_DIR") else {
        return;
    };
    // .../target/<profile>/build/spotify-rust-<hash>/out -> .../target/<profile>
    let Some(profile) = std::path::Path::new(&out).ancestors().nth(3) else {
        return;
    };
    let _ = std::fs::copy(dll, profile.join("WebView2Loader.dll"));
}
