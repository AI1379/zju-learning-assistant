fn main() {
    let gui_enabled = std::env::var_os("CARGO_FEATURE_GUI").is_some();
    let custom_protocol_enabled = std::env::var_os("CARGO_FEATURE_CUSTOM_PROTOCOL").is_some();

    if gui_enabled || custom_protocol_enabled {
        #[cfg(feature = "tauri-build-support")]
        tauri_build::build()
    }
}
