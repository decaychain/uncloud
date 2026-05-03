const COMMANDS: &[&str] = &[
    "status",
    "is_enrolled",
    "enroll",
    "unlock",
    "clear",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
