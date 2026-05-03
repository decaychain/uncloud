//! Android-only biometric unlock plugin.
//!
//! All command handlers live in Kotlin (`BiometricPlugin.kt`); the Rust
//! side only registers the native plugin so JS `invoke('plugin:uncloud-biometric|...')`
//! can reach it. On non-Android platforms the plugin is a no-op — the
//! frontend's `hooks/biometric.rs` shim short-circuits before invoking.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Runtime,
};

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "de.lunarstream.uncloud.biometric";

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("uncloud-biometric")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                let _ = _api.register_android_plugin(PLUGIN_IDENTIFIER, "BiometricPlugin")?;
            }
            Ok(())
        })
        .build()
}
