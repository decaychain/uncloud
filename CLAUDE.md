# Uncloud — Project Guide

Uncloud is a self-hosted personal cloud storage system. It is a Rust workspace with six crates:

- `crates/uncloud-server` — Axum HTTP server, REST API + S3-compatible API + app reverse-proxy, MongoDB, local file storage
- `crates/uncloud-web` — Dioxus 0.7 WASM frontend, Tailwind CSS + DaisyUI
- `crates/uncloud-common` — Shared types (API request/response structs, `ServerEvent`, `ApiClient`, validation), re-exported to both server and web
- `crates/uncloud-client` — Native HTTP client (reqwest with cookie jar) for the desktop app
- `crates/uncloud-sync` — Two-way file sync engine with SQLite journal + activity broadcast (used by the desktop app)
- `crates/uncloud-desktop` — Tauri v2 desktop app (tray-only, bundles the Dioxus web frontend, filesystem watcher, autostart)

## Documentation

| Topic | File |
|---|---|
| Git workflow, CI, dev commands | [docs/Workflow.md](docs/Workflow.md) |
| Repository layout, key conventions, storage design, API routes, config reference | [docs/Architecture.md](docs/Architecture.md) |
| Implemented feature inventory | [docs/Features.md](docs/Features.md) |
| Outstanding work / planned features | [docs/Roadmap.md](docs/Roadmap.md) |
| Design notes (per-feature) | `docs/*.md` (kebab-case files) |

When you change behaviour that affects any of the above, update the relevant doc in the same change. Keep this index slim — it is auto-loaded into every Claude session, so detail belongs in the linked files.

## Critical conventions Claude must always apply

These are the conventions most likely to bite if forgotten. Full set lives in [docs/Architecture.md → Key Conventions](docs/Architecture.md#key-conventions).

- **Don't merge PRs.** For large features, open a PR with `gh pr create --fill --base main` and **stop**. The user manually tests the branch and merges. Small fixes (bug fixes, doc updates, config tweaks, CI adjustments) commit directly to `main` — no branch, no PR. See [docs/Workflow.md](docs/Workflow.md) for the full rules.
- **Auth**: cookie sessions OR `Authorization: Bearer <token>`. Public routes are `/api/auth/*` and `/api/public/*`; everything else needs auth.
- **Versioned API**: every authenticated route is mounted under both `/api/...` and `/api/v1/...`. Use `/api/v1/` for any external consumer. v1-only routes: `auth/tokens`, `s3/credentials`, `apps`, `auth/me/features`, `auth/me/preferences`.
- **Datetime storage**: model fields use `chrono::DateTime<Utc>` annotated with `#[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]`. `Option<DateTime<Utc>>` uses the custom `models::opt_dt` serde module. This stores proper BSON Date objects without dragging MongoDB types into domain models.
- **Audit log**: change-inducing handlers must call `audit::file_event` / `audit::folder_event` to append a `SyncEvent` row — also broadcast as `ServerEvent::SyncEventAppended`.
- **Mobile safe-area**: any `fixed inset-0` / `fixed top-0` / `fixed bottom-0` overlay must add `env(safe-area-inset-top)` / `env(safe-area-inset-bottom)` so content doesn't sit under Android system bars. Use `pt-safe` / `pb-safe` utilities or inline `style: "padding-top: calc(<base> + env(safe-area-inset-top))"`. Prefer the shared `RightDrawer` component for right-side panels — it already handles this.
- **Don't write emojis** in code, comments, or commit messages unless the user explicitly asks.
- **Don't write comments that explain WHAT the code does.** Only comment when the WHY is non-obvious (hidden constraint, subtle invariant, workaround for a specific bug).
