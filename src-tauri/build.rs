fn main() {
    // Set by the release workflow: only its builds look for updates (see `window::updates_on`).
    println!("cargo:rerun-if-env-changed=RELIGHT_UPDATES");
    tauri_build::build()
}
