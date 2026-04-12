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

## Getting Started

### Prerequisites

- **Rust** (stable toolchain) — [rustup.rs](https://rustup.rs)
- **MongoDB** — running instance (default: `localhost:27017`)
- **Meilisearch** (optional) — enables full-text search

### Configure

```bash
cp config.example.yaml config.yaml
# Edit config.yaml — at minimum, set storage.default_path to a writable directory
```

### Create the first admin user

```bash
cargo run -p uncloud-server -- bootstrap-admin --username admin --password yourpassword
```

If you omit `--password`, a secure random one is generated and printed:

```bash
cargo run -p uncloud-server -- bootstrap-admin --username admin
# Admin user created successfully.
#   Username: admin
#   Password: 7a3f...  (save this!)
```

You can also pass `--email admin@example.com`.

### Run the server

```bash
cargo run -p uncloud-server
```

Listens on `http://localhost:8080` by default (configurable in `config.yaml`).

### Web frontend (dev mode)

```bash
cd crates/uncloud-web
npx tailwindcss -i input.css -o assets/tailwind.css --watch  # Terminal 1
dx serve                                                       # Terminal 2
```

The dev frontend proxies API requests to the server at `localhost:8080`.

## Pre-built Binaries

### Docker Compose

```bash
cp config.example.yaml config.yaml
docker compose up -d
```

### From Binary

Download `uncloud-server-linux-amd64` from [Releases](https://github.com/decaychain/uncloud/releases):

```bash
chmod +x uncloud-server-linux-amd64
./uncloud-server-linux-amd64 bootstrap-admin --username admin
./uncloud-server-linux-amd64
```

### Desktop / Android

Download from [Releases](https://github.com/decaychain/uncloud/releases).

## Building from Source

```bash
# Server
cargo run -p uncloud-server

# Web frontend (dev mode — see above)

# Desktop (Linux: requires webkit2gtk4.1-devel, libsoup3-devel)
./build-desktop.sh

# Android
./build-android.sh
```

## Configuration

See [`config.example.yaml`](config.example.yaml) for all available options.

## License

[MIT](LICENSE)
