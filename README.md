# Uncloud

Self-hosted personal cloud storage. Manage your files, photos, and music from any device — all data stays on your server.

## Features

- **File management** — upload, download, move, copy, rename, bulk operations
- **Version history** — automatic versioning with restore
- **Trash** — soft delete with auto-purge
- **Photo gallery** — timeline view and album organization
- **Music library** — artist/album browsing, playlists, in-browser playback
- **Full-text search** — powered by Meilisearch
- **Password manager** — KeePass-compatible (.kdbx) vault with client-side encryption, entry/group management, and password generator
- **Sharing** — public links with optional password, expiry, and download limits
- **Desktop sync** — two-way file sync with per-folder strategy
- **Multi-platform** — web, desktop (Linux/Windows), and Android

## Architecture

- **Server**: Rust (Axum + MongoDB + local filesystem)
- **Web frontend**: Dioxus 0.7 (Rust/WASM) + Tailwind CSS + DaisyUI
- **Desktop**: Tauri v2 (bundles the web frontend)
- **Android**: Tauri v2 (Android WebView)

## Quick Start

### Docker Compose

```bash
cp config.example.yaml config.yaml
# Edit config.yaml with your settings
docker compose up -d
```

The server will be available at `http://localhost:8080`.

### From Binary

Download the latest `uncloud-server-linux-amd64` from [Releases](https://github.com/decaychain/uncloud/releases), then:

```bash
chmod +x uncloud-server-linux-amd64
cp config.example.yaml config.yaml
# Edit config.yaml
./uncloud-server-linux-amd64
```

Requires a running MongoDB instance (see `config.yaml` for connection settings).

### Desktop App

Download the `.deb`, `.rpm`, or `.msi` from [Releases](https://github.com/decaychain/uncloud/releases).

### Android

Download the APK from [Releases](https://github.com/decaychain/uncloud/releases) or from Google Play (internal testing).

## Building from Source

```bash
# Server
cargo run -p uncloud-server

# Web frontend (dev mode)
cd crates/uncloud-web
npx tailwindcss -i input.css -o assets/tailwind.css --watch  # Terminal 1
dx serve                                                       # Terminal 2

# Desktop (Linux: requires webkit2gtk4.1-devel, libsoup3-devel)
./build-desktop.sh

# Android
./build-android.sh
```

## Configuration

See [`config.example.yaml`](config.example.yaml) for all available options.

## License

[MIT](LICENSE)
