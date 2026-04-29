# Roadmap

Outstanding work and planned features.

Both major items from the original roadmap (App Platform, S3-Compatible API) have shipped — see [Features.md](Features.md). Remaining gaps:

## More storage backends

- `LocalStorage` (filesystem) and `S3Storage` (any S3-compatible service) ship today. Admins can configure multiple storages and route different folders/files to different backends.
- A second non-S3 backend would be SFTP — works against any VPS / NAS, supports random reads, no SaaS lock-in. SCP is a strict subset (no listing, no random reads) and not viable.
- WebDAV and SMB are not planned: WebDAV is glitchy in practice and SMB is better mounted at the OS level than implemented as a backend.
- A `MirrorBackend` wrapping a primary plus N read-only secondaries (for off-site backup) is a possible future addition, but currently each file lives on exactly one backend.

## At-rest encryption

- Storage is currently plaintext on disk.
- Options: per-user master key with envelope encryption, or server-wide key with per-file IVs; transparent encrypt/decrypt at the `StorageBackend` layer.

## Passkeys / WebAuthn

- TOTP is implemented; WebAuthn would be a stronger second factor (or first factor, replacing passwords) and avoid TOTP secret-leak / phishing concerns.

## App Platform polish

- `App.enabled_for: Vec<ObjectId>` is in the model but there's no admin UI to gate apps per-user (currently apps are visible to everyone).
- No dev story yet for how a sidecar app authenticates user actions (presumably it'd use `/api/v1/auth/tokens` minted per-user, but the round-trip is undocumented).
