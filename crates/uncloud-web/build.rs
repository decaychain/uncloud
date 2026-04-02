use std::path::Path;
use std::process::Command;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let crate_dir = Path::new(&manifest_dir);

    // Re-run this script whenever Rust sources, the CSS input, or the
    // Tailwind config change so the generated CSS stays in sync.
    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=input.css");
    println!("cargo:rerun-if-changed=tailwind.config.js");
    println!("cargo:rerun-if-changed=index.html");

    // Install npm dependencies the first time (or if node_modules is missing).
    let node_modules = crate_dir.join("node_modules");
    if !node_modules.exists() {
        let status = Command::new("npm")
            .arg("install")
            .current_dir(crate_dir)
            .status()
            .expect("Failed to run `npm install` — is npm installed?");

        if !status.success() {
            panic!("`npm install` exited with a non-zero status");
        }
    }

    // Generate assets/tailwind.css from input.css.
    let status = Command::new("npx")
        .args(["--no-install", "tailwindcss", "-i", "input.css", "-o", "assets/tailwind.css"])
        .current_dir(crate_dir)
        .status()
        .expect("Failed to run `npx tailwindcss` — is npm installed?");

    if !status.success() {
        panic!("`npx tailwindcss` exited with a non-zero status");
    }
}
