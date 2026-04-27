# Roadmap

Outstanding work and planned features.

Both major items from the original roadmap (App Platform, S3-Compatible API) have shipped — see [Features.md](Features.md). Remaining gaps:

## Multi-storage backends

- `LocalStorage` (filesystem) is the only `StorageBackend` implementation.
- The next concrete backend to add is an S3 client (Backblaze B2 / Cloudflare R2 / AWS S3) for cheap off-site mirroring; WebDAV and SMB are also plausible.
- Note: this is the **outbound** S3 client, distinct from the inbound S3-compatible API that's already implemented.

## At-rest encryption

- Storage is currently plaintext on disk.
- Options: per-user master key with envelope encryption, or server-wide key with per-file IVs; transparent encrypt/decrypt at the `StorageBackend` layer.

## Passkeys / WebAuthn

- TOTP is implemented; WebAuthn would be a stronger second factor (or first factor, replacing passwords) and avoid TOTP secret-leak / phishing concerns.

## App Platform polish

- `App.enabled_for: Vec<ObjectId>` is in the model but there's no admin UI to gate apps per-user (currently apps are visible to everyone).
- No dev story yet for how a sidecar app authenticates user actions (presumably it'd use `/api/v1/auth/tokens` minted per-user, but the round-trip is undocumented).
