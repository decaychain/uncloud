# Mail Client Foundation

Experimental design for adding a first-party email client to Uncloud. The goal
is a mail client that connects to external providers over IMAP/SMTP, not a mail
server.

Status: experimental branch `experimental-mail-foundation`. The foundation has
been manually verified against a real IMAP account: account creation works,
`test-imap` authenticates successfully, and `folders/refresh` retrieves and
stores the remote folder list.

## Scope

The foundation supports:

- Multiple mail accounts per Uncloud user.
- Multiple send identities per account.
- IMAP folders and subfolders, stored by remote path plus hierarchy delimiter.
- Message and attachment metadata models for a cached mailbox.
- A provider boundary backed initially by `async-imap`.
- SMTP/MIME/sanitization dependencies selected for later send and render work:
  `lettre`, `mail-parser`, and `ammonia`.

The first implementation now supports encrypted-at-rest IMAP password storage
behind a deployment master key. Endpoints that need provider access still accept
a transient password for manual testing, but can also use the stored account
credential when the request body omits `password`.

## Backend Layout

- Shared API types: `crates/uncloud-common/src/api/mail.rs`
- Mongo models: `crates/uncloud-server/src/models/mail.rs`
- HTTP routes: `crates/uncloud-server/src/routes/mail.rs`
- Provider/service layer: `crates/uncloud-server/src/services/mail.rs`

Collections:

- `mail_accounts`
- `mail_identities`
- `mail_folders`
- `mail_messages`
- `mail_attachments`

## API Surface

Authenticated routes are mounted under both `/api` and `/api/v1`:

- `GET /mail/accounts`
- `POST /mail/accounts`
- `PUT /mail/accounts/{id}`
- `DELETE /mail/accounts/{id}`
- `GET /mail/accounts/{id}/credential`
- `PUT /mail/accounts/{id}/credential`
- `DELETE /mail/accounts/{id}/credential`
- `POST /mail/accounts/{id}/test-imap`
- `GET /mail/accounts/{account_id}/folders`
- `POST /mail/accounts/{account_id}/folders/refresh`
- `GET /mail/identities`
- `POST /mail/identities`
- `PUT /mail/identities/{id}`
- `DELETE /mail/identities/{id}`

`test-imap` and `folders/refresh` currently require implicit TLS IMAP. STARTTLS
and plaintext ports are represented in the data model but not wired yet.

## What Works

- Creating, listing, updating, and deleting mail account metadata.
- Creating, listing, updating, and deleting sender identities.
- Multiple accounts per user and multiple identities per account.
- Encrypted-at-rest IMAP password storage using `secrets.master_key`.
- Credential status is exposed only as `credential_configured: true/false`.
- IMAP implicit TLS login with a transient password.
- IMAP implicit TLS login with a stored account credential.
- IMAP capability retrieval through `POST /mail/accounts/{id}/test-imap`.
- IMAP folder discovery through `POST /mail/accounts/{account_id}/folders/refresh`.
- Folder/subfolder persistence using remote path, hierarchy delimiter, parent
  path, attributes, and selectable state.
- Mongo indexes for account, identity, folder, message, and attachment metadata.
- Server-wide `features.mail` toggle plus per-user opt-out through
  `disabled_features`.

## Known Limits

- Stored credentials currently cover IMAP app passwords only. OAuth refresh
  tokens and SMTP credential handling still need a credential type model.
- Only implicit TLS IMAP is wired. STARTTLS and plaintext are represented in the
  API/model but return a validation error in the provider layer.
- SMTP is not wired beyond selecting `lettre` as the planned foundation.
- No message sync yet. `mail_messages` and `mail_attachments` are model/index
  scaffolding only.
- No MIME parsing or HTML sanitization path is wired yet.
- No UI yet. Testing is currently through authenticated HTTP calls.
- Folder refresh currently replaces cached folder rows for the account. That is
  acceptable before message sync, but once messages reference folders we should
  upsert folders by `(owner_id, account_id, path)` to preserve stable folder ids.

## End-to-End Test Path

1. Start `uncloud-server` with MongoDB available.
2. Log in and save the session cookie.
3. `POST /api/mail/accounts` with IMAP/SMTP settings.
4. Optionally `PUT /api/mail/accounts/{id}/credential` with an app password.
5. `POST /api/mail/accounts/{id}/test-imap` with either a transient app
   password or `{}` to use the stored credential.
6. `POST /api/mail/accounts/{id}/folders/refresh` the same way.
7. `GET /api/mail/accounts/{id}/folders` and verify folders/subfolders are
   persisted with expected paths and delimiters.

This is enough to validate the current protocol foundation before building UI.

## Next Work

### 1. Credential Storage

Initial encrypted server-side secret storage is in place:

- `secrets.master_key` is a base64-encoded 32-byte deployment secret.
- IMAP passwords are encrypted with AES-256-GCM before writing to MongoDB.
- Account responses return only `credential_configured: true/false`.
- Routes exist to inspect status, set/replace, and clear account credentials.
- `test-imap` and `folders/refresh` use a transient password when provided, or
  the stored credential when `password` is omitted.

Remaining credential work before scheduler/background sync:

- Add credential types for OAuth refresh tokens and any SMTP-specific password
  split if providers need separate IMAP/SMTP secrets.
- Decide the rotation story for `secrets.master_key`.
- Consider a small admin/health check that reports whether mail credential
  storage is configured without exposing the key.

### 2. Provider Capability

- Add STARTTLS support for IMAP port 143.
- Decide whether plaintext IMAP should remain supported at all. If yes, keep it
  explicit and warn loudly.
- Add SMTP connection/authentication testing with `lettre`.
- Normalize provider error mapping so bad credentials return a clear 400/401-ish
  response while network/server failures remain operational errors.

### 3. Read-Only Message Sync

- Change folder refresh to upsert folders and keep stable ids.
- Add per-folder sync state: UIDVALIDITY, UIDNEXT, highest synced UID, last sync
  timestamps, and last error.
- On UIDVALIDITY change, invalidate cached messages for that folder and resync.
- Fetch message envelopes/flags/internal dates first.
- Fetch raw RFC822 bodies only when needed, or in bounded batches.
- Store raw messages and large decoded parts in Uncloud storage, not MongoDB.
- Keep MongoDB as searchable/listable metadata cache.

### 4. Message Read APIs

- Add message list route with pagination by folder and date/UID.
- Add message detail route that parses the stored raw message with
  `mail-parser`.
- Sanitize HTML with `ammonia` before returning/rendering it.
- Add attachment metadata and download routes.
- Decide how remote image loading should work. Default should be blocked or
  proxied, not direct by default.

### 5. Mutations

- Mark read/unread, star/unstar, archive, move, delete.
- Reflect remote flag/folder changes locally.
- Make mutations idempotent where possible and record failures clearly.
- Defer complex threading until list/detail sync is solid.

### 6. Sending

- Add compose/send route using an identity and SMTP settings.
- Support plain text plus HTML body.
- Save sent copy through provider conventions: SMTP server may do it, but IMAP
  append to Sent may still be needed for some providers.
- Add reply/forward metadata handling.
- Add draft storage after basic send works.

### 7. UI

Do not build a Gmail-like shell before read-only message sync exists. The first
useful UI should be:

- Account setup and connection test.
- Folder list.
- Read-only message list.
- Message reader.

After that, add compose and mutations. A full Gmail/Proton-like layout becomes
worth polishing once the backend cache can serve real message lists reliably.

## Open Questions

- Should mail credentials use a server-wide master key, per-user key material,
  or both?
- Should OAuth be a first-class credential type in v1, or should v1 focus on app
  passwords/standard IMAP first?
- Which storage backend should hold raw RFC822 bodies and attachments by
  default: the user's resolved Uncloud storage, a dedicated mail storage prefix,
  or Mongo GridFS?
- Should message full-text search use Meilisearch immediately, or wait until the
  cache/mutation model stabilizes?
- How much provider-specific behavior do we want for Gmail/Outlook/Fastmail
  versus a generic IMAP implementation?
