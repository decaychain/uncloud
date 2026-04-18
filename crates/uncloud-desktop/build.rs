use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    tauri_build::build();
    embed_fallback_key();
}

/// Emit a `fallback_key.rs` to `OUT_DIR` containing a 32-byte AES-256 key
/// constant. The key comes from the `UNCLOUD_DESKTOP_FALLBACK_KEY` env var
/// (hex-encoded, 64 chars) at build time. Release builds without it fail to
/// compile; debug builds fall back to a well-known dev key (which only ever
/// touches the `uncloud-dev` config namespace).
fn embed_fallback_key() {
    println!("cargo:rerun-if-env-changed=UNCLOUD_DESKTOP_FALLBACK_KEY");

    let key = match env::var("UNCLOUD_DESKTOP_FALLBACK_KEY") {
        Ok(hex) => parse_hex_key(&hex)
            .unwrap_or_else(|e| panic!("UNCLOUD_DESKTOP_FALLBACK_KEY: {e}")),
        Err(_) => {
            let profile = env::var("PROFILE").unwrap_or_default();
            if profile == "release" {
                panic!(
                    "UNCLOUD_DESKTOP_FALLBACK_KEY must be set for release builds \
                     (hex-encoded 32 bytes / 64 chars). It is the encryption key \
                     used to fall back to disk storage when the OS keyring is \
                     unavailable."
                );
            }
            *b"uncloud-desktop-dev-fallback-key"
        }
    };

    let mut src = String::from("pub const FALLBACK_KEY: [u8; 32] = [");
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
