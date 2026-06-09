# Uncloud

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Latest Release](https://img.shields.io/github/v/release/decaychain/uncloud?include_prereleases&sort=semver)](https://github.com/decaychain/uncloud/releases)
[![Fedora COPR](https://img.shields.io/badge/Fedora-COPR-294172?logo=fedora&logoColor=white)](https://copr.fedorainfracloud.org/coprs/decaychain/uncloud/)
[![Ubuntu APT](https://img.shields.io/badge/Ubuntu-APT_repo-E95420?logo=ubuntu&logoColor=white)](https://decaychain.github.io/uncloud/)
[![Google Play](https://img.shields.io/badge/Google_Play-Android-3DDC84?logo=googleplay&logoColor=white)](https://play.google.com/store/apps/details?id=de.lunarstream.uncloud)

Self-hosted personal cloud. Manage files, photos, music, mail, passwords, tasks, shopping lists, and personal finances from your own server.

## Features

- **Files and photos** — upload, download, move, copy, rename, bulk operations, thumbnails, timeline gallery, albums, version history, trash, and sharing links
- **Music library** — artist/album browsing, folder-based categories, lazy folder browser, playlists, in-browser playback, native background playback on supported clients, and Subsonic/OpenSubsonic-compatible access
- **Mail client** — multi-account IMAP/SMTP client with folder roles, scheduled sync, HTML rendering, rich-text compose, identities, attachments, and bulk actions
- **Finance tracker** — bank CSV imports, accounts, categories/subcategories, rule-based categorization, reconciliation entries, summaries, and mobile-friendly transaction review
- **Tasks and projects** — project boards, list/schedule views, labels, sections, subtasks, checklists, comments, and sharing
- **Shopping lists** — reusable item catalogue, shops/categories, recurring items, drag-to-reorder lists, and shared lists
- **Password manager** — KeePass-compatible (.kdbx) vault with client-side encryption, entry/group management, password generator, TOTP fields, and Android biometric unlock
- **Full-text search** — powered by Meilisearch
- **Desktop sync** — two-way file sync with per-folder strategy
- **Pluggable storage** — local filesystem, any S3-compatible service (AWS S3, Backblaze B2, Cloudflare R2, MinIO), or any SSH-accessible host (SFTP); mix and match per folder
- **S3-compatible API** — point `s5cmd`, `rclone`, `aws-cli`, or any SigV4 client at Uncloud
- **MCP server** — connect Claude.ai (or any Model Context Protocol client) to your files via OAuth 2.1; tools for listing, reading, searching, creating, writing, moving, copying, and deleting
- **Encrypted backups** — `uncloud-server backup` writes deduplicated, encrypted snapshots (database + file blobs) to a Restic-format repository (SFTP, S3, B2, Azure, GCS, REST, or local)
- **Optional apps** — Mail, Finance, Tasks, Shopping, and Music can be enabled or disabled per server and per user
- **Multi-platform** — web, desktop (Linux/Windows), and Android

## Architecture

- **Server**: Rust (Axum + MongoDB; pluggable storage: local / S3 / SFTP)
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

Static binaries for `linux-amd64` and `linux-arm64` are published on [Releases](https://github.com/decaychain/uncloud/releases):

```bash
chmod +x uncloud-server-linux-amd64
./uncloud-server-linux-amd64 bootstrap-admin --username admin
./uncloud-server-linux-amd64
```

### Desktop client

The desktop client is published through three channels — pick the one
that matches your distribution:

**Fedora** (auto-updates via `dnf upgrade`):

```bash
sudo dnf copr enable decaychain/uncloud
sudo dnf install uncloud
```

**Ubuntu / Debian** (auto-updates via `apt upgrade`):

```bash
curl -fsSL https://decaychain.github.io/uncloud/pubkey.gpg \
  | sudo gpg --dearmor -o /usr/share/keyrings/uncloud.gpg
echo "deb [signed-by=/usr/share/keyrings/uncloud.gpg] https://decaychain.github.io/uncloud stable main" \
  | sudo tee /etc/apt/sources.list.d/uncloud.list
sudo apt update
sudo apt install uncloud
```

**Windows** (auto-updates via the in-app updater): download the signed
NSIS installer from [Releases](https://github.com/decaychain/uncloud/releases).

**Escape hatch** — raw `.deb` / `.rpm` / `.exe` for both `amd64` and
`arm64` are also attached to every [release](https://github.com/decaychain/uncloud/releases).

### Android

Install Uncloud from Google Play:

[<img src="https://play.google.com/intl/en_us/badges/static/images/badges/en_badge_web_generic.png" alt="Get it on Google Play" height="64">](https://play.google.com/store/apps/details?id=de.lunarstream.uncloud)

For sideloading, the universal `.apk` is also attached to each
[release](https://github.com/decaychain/uncloud/releases).

## Building from Source

```bash
# Server
cargo run -p uncloud-server

# Web frontend (dev mode — see above)

# Desktop (Linux: requires webkit2gtk4.1-devel, libsoup3-devel, libappindicator-gtk3-devel)
./build-desktop.sh

# Android
./build-android.sh
```

## Configuration

See [`config.example.yaml`](config.example.yaml) for all available options.

## Subsonic-compatible Music Clients

Uncloud exposes a Subsonic/OpenSubsonic-compatible API at `/rest/...` for
external music players such as Symfonium and Feishin. Enable Music for the
user, include folders in the Music library, then create a Subsonic app password
from Settings. Use your Uncloud server URL, username, and that app password in
the client.

The API streams original files from your library. Transcoding is deliberately
not supported.

## Connecting an MCP client

Uncloud exposes a Model Context Protocol endpoint at `/mcp`, gated by
OAuth 2.1 with PKCE. To connect Claude.ai, MCP Inspector, or
`claude mcp add`, point the client at the **full URL including `/mcp`**:

```
https://your-uncloud-host/mcp
```

The client will walk OAuth discovery, register itself, and run you
through a consent screen on your Uncloud instance. See
[`docs/mcp.md`](docs/mcp.md) for the full setup guide, tool reference,
and reverse-proxy header requirements.

## License

[MIT](LICENSE)
