# Roadmap

Outstanding work and planned features.

Both major items from the original roadmap (App Platform, S3-Compatible API) have shipped — see [Features.md](Features.md). Remaining gaps:

## More storage backends

- `LocalStorage` (filesystem), `S3Storage` (any S3-compatible service), and `SftpStorage` (any SSH-accessible host) ship today. Admins configure multiple storages in `config.yaml` and route different folders/files to different backends.
- WebDAV and SMB are not planned: WebDAV is glitchy in practice and SMB is better mounted at the OS level than implemented as a backend.
- A `MirrorBackend` wrapping a primary plus N read-only secondaries (for off-site backup) is a possible future addition, but currently each file lives on exactly one backend.

## Offline storage migration

- Per-folder storage pinning routes new uploads, but existing files stay on whatever backend they were uploaded to. There is no way today to move a folder's history to a different backend.
- Planned as an offline `uncloud-server migrate --from <id> --to <id>` subcommand: server stopped, per-file copy + atomic pointer flip, idempotent and resumable. Design: [storage-migration.md](storage-migration.md).

## Backup to remote repos

- File contents and database state have no unified backup story today — native per-backend backups miss the database, and `mongodump` alone is a list of dangling pointers.
- Planned as an `uncloud-server backup create --target <name>` subcommand that writes a single deduplicated, encrypted snapshot to a Restic-format repository (via `rustic_core`). Snapshot contains a semantic NDJSON dump of the database (engine-neutral) plus all file blobs organised by logical path. In-place `backup restore` matches storages by name (with default-storage fallback) so DR works whether you're rolling back an existing install or rebuilding on fresh hardware. Multiple targets configurable in `config.yaml`; supports SFTP, S3, B2, Azure, GCS, REST, and local repos. Design: [backup.md](backup.md).

## At-rest encryption

- Storage is currently plaintext on disk.
- Options: per-user master key with envelope encryption, or server-wide key with per-file IVs; transparent encrypt/decrypt at the `StorageBackend` layer.

## Passkeys / WebAuthn

- TOTP is implemented; WebAuthn would be a stronger second factor (or first factor, replacing passwords) and avoid TOTP secret-leak / phishing concerns.

## App Platform polish

- `App.enabled_for: Vec<ObjectId>` is in the model but there's no admin UI to gate apps per-user (currently apps are visible to everyone).
- No dev story yet for how a sidecar app authenticates user actions (presumably it'd use `/api/v1/auth/tokens` minted per-user, but the round-trip is undocumented).
