use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    tauri_build::build();
    embed_fallback_key();
}

/// Emit a `fallback_key.rs` to `OUT_DIR` containing a 32-byte AES-256 key
/// constant plus a `FALLBACK_KEY_PROVIDED` bool flag. The key comes from
/// the `UNCLOUD_DESKTOP_FALLBACK_KEY` env var (hex-encoded, 64 chars).
///
/// When the env var is set (GitHub release builds with the repo secret),
/// the key is embedded and `FALLBACK_KEY_PROVIDED = true`. The runtime
/// uses it as the seed for the per-installation key file on first run
/// — this keeps existing installs decryptable across upgrades.
///
/// When the env var is not set (COPR mock builds, anyone building from
/// source), a sentinel of all-zeros is embedded and `FALLBACK_KEY_PROVIDED
/// = false`. The runtime ignores the sentinel and generates a fresh
/// random per-install key on first need, persisted to disk so subsequent
/// upgrades reuse the same key (no re-auth on every release).
///
/// Debug builds with no env var get a well-known dev key (only ever
/// touches the `uncloud-dev` config namespace).
fn embed_fallback_key() {
    println!("cargo:rerun-if-env-changed=UNCLOUD_DESKTOP_FALLBACK_KEY");

    let (key, provided) = match env::var("UNCLOUD_DESKTOP_FALLBACK_KEY") {
        Ok(hex) => (
            parse_hex_key(&hex).unwrap_or_else(|e| panic!("UNCLOUD_DESKTOP_FALLBACK_KEY: {e}")),
            true,
        ),
        Err(_) => {
            let profile = env::var("PROFILE").unwrap_or_default();
            if profile == "release" {
                ([0u8; 32], false)
            } else {
                (*b"uncloud-desktop-dev-fallback-key", true)
            }
        }
    };

    let mut src = String::new();
    src.push_str(&format!(
        "pub const FALLBACK_KEY_PROVIDED: bool = {};\n",
        provided
    ));
    src.push_str("pub const FALLBACK_KEY: [u8; 32] = [");
    for b in key.iter() {
        src.push_str(&format!("{}u8,", b));
    }
    src.push_str("];\n");

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"))
        .join("fallback_key.rs");
    fs::write(&out, src).expect("failed to write fallback_key.rs");
}

fn parse_hex_key(hex: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", hex.len()));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let chunk = &hex[i * 2..i * 2 + 2];
        *byte = u8::from_str_radix(chunk, 16)
            .map_err(|e| format!("invalid hex at byte {i}: {e}"))?;
    }
    Ok(out)
}
