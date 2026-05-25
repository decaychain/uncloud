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
- `POST /mail/accounts/{id}/test-smtp`
- `POST /mail/accounts/{id}/send`
- `POST /mail/accounts/{id}/sync`
- `GET /mail/accounts/{account_id}/folders`
- `POST /mail/accounts/{account_id}/folders/refresh`
- `PUT /mail/accounts/{account_id}/folders/{folder_id}`
- `POST /mail/accounts/{account_id}/folders/{folder_id}/sync`
- `GET /mail/accounts/{account_id}/folders/{folder_id}/messages`
- `GET /mail/messages/{message_id}`
- `POST /mail/messages/{message_id}/mutate`
- `GET /mail/identities`
- `POST /mail/identities`
- `PUT /mail/identities/{id}`
- `DELETE /mail/identities/{id}`

`test-imap` and `folders/refresh` support implicit TLS, STARTTLS, and explicit
plaintext IMAP. Plaintext is intended only for trusted local/testing setups.
`test-smtp` supports implicit TLS, STARTTLS, and explicit plaintext SMTP using
the account's SMTP settings.

## What Works

- Creating, listing, updating, and deleting mail account metadata.
- Creating, listing, updating, and deleting sender identities.
- Multiple accounts per user and multiple identities per account.
- Encrypted-at-rest IMAP password storage using `secrets.master_key`.
- Credential status is exposed only as `credential_configured: true/false`.
- IMAP implicit TLS, STARTTLS, or plaintext login with a transient password.
- IMAP implicit TLS, STARTTLS, or plaintext login with a stored account
  credential.
- IMAP capability retrieval through `POST /mail/accounts/{id}/test-imap`.
- IMAP folder discovery through `POST /mail/accounts/{account_id}/folders/refresh`.
- SMTP connection/authentication testing through
  `POST /mail/accounts/{id}/test-smtp`.
- Folder/subfolder persistence using remote path, hierarchy delimiter, parent
  path, attributes, selectable state, role mapping, and account-sync inclusion.
- Folder refresh upserts by `(owner_id, account_id, path)`, preserving stable
  folder ids across refreshes.
- Folder role inference marks common Inbox, Sent, Drafts, Archive, Trash, Spam,
  and All Mail folders from IMAP attributes and common folder names. Manual role
  overrides are preserved across folder refreshes.
- Manual read-only message summary sync for one folder or all cached selectable
  folders included in account sync. Each sync fetches a bounded UID window of
  envelope, flags, internal date, and RFC822 size metadata.
- Per-folder sync state: UIDVALIDITY, UIDNEXT, exists/unseen counts, highest
  scanned UID, sync timestamps, and last error.
- UIDVALIDITY changes invalidate cached message summaries for that folder before
  resync.
- Basic message summary listing by folder from the MongoDB cache.
- On-demand message detail/body fetch for the reader pane. This currently uses
  the stored credential, fetches `BODY.PEEK[]`, parses MIME with `mail-parser`,
  and returns plain text plus server-sanitized HTML.
- Manual message mutations for the first write-path spike: mark read/unread,
  star/unstar, move to a selected folder, archive, and move to trash.
- Minimal SMTP send through a selected identity or account fallback. The first
  compose path sends plain text only and uses the stored account credential.
- Sent-copy handling for basic sends: after SMTP accepts the message, Uncloud
  checks the configured Sent folder by `Message-ID`; if the provider did not
  save it, Uncloud appends the exact RFC822 payload to Sent.
- First experimental web UI iteration at `/mail`: account/folder navigation,
  account setup/settings, IMAP/SMTP tests, folder settings, manual
  account/folder sync, cached message list, reader pane, and basic message
  mutation/compose controls.
- Mongo indexes for account, identity, folder, message, and attachment metadata.
- Server-wide `features.mail` toggle plus per-user opt-out through
  `disabled_features`.

## Known Limits

- Stored credentials currently cover IMAP app passwords only. OAuth refresh
  tokens and SMTP credential handling still need a credential type model.
- SMTP is wired for connection/authentication testing and plain-text send.
- Message sync currently stores summaries only. Raw RFC822 bodies, decoded
  parts, parsed body output, attachment persistence, and sanitized HTML are not
  persisted yet.
- Body rendering is on-demand. The UI prefers sanitized HTML when available and
  falls back to plain text. Remote image URLs are stripped during sanitization.
- Move/archive/trash currently require provider support for `UID MOVE`. There is
  deliberately no copy-plus-expunge fallback yet, because expunge semantics can
  be risky across clients.
- After a successful move/archive/trash, the source cached message is removed.
  The destination copy is discovered by the next sync because IMAP move changes
  the destination UID and the foundation does not yet consume UIDPLUS response
  codes.
- Compose, search, threading, attachments, permanent delete, and draft handling
  are not fully implemented. Compose currently means "send a plain-text message
  now" with no drafts, attachments, rich editor, reply/forward headers, or
  provider-specific sent-copy policy.
- Sent-copy detection is intentionally conservative. If checking the Sent folder
  fails, Uncloud reports the failure and does not append, to avoid creating a
  duplicate when the provider may have saved the message already.
- Account-level manual sync refreshes folders first, then syncs selectable
  folders one-by-one. It is intentionally simple and not yet a scheduler.

## End-to-End Test Path

1. Start `uncloud-server` with MongoDB available.
2. Log in and save the session cookie.
3. `POST /api/mail/accounts` with IMAP/SMTP settings.
4. Optionally `PUT /api/mail/accounts/{id}/credential` with an app password.
5. `POST /api/mail/accounts/{id}/test-imap` with either a transient app
   password or `{}` to use the stored credential.
6. `POST /api/mail/accounts/{id}/test-smtp` the same way.
7. Optionally `POST /api/mail/accounts/{id}/send` with a small plain-text test
   message.
8. `POST /api/mail/accounts/{id}/folders/refresh` the same way.
9. `GET /api/mail/accounts/{id}/folders` and verify folders/subfolders are
   persisted with expected paths and delimiters.
10. `POST /api/mail/accounts/{id}/sync` with `{}` to use the stored credential,
   or include `password` and optional `limit_per_folder` for manual testing.
11. `GET /api/mail/accounts/{account_id}/folders/{folder_id}/messages` to
    inspect cached message summaries for a synced folder.
12. Open `/mail` in the web UI and verify account/folder selection, sync
    controls, message list, and sanitized HTML/plain-text reader body.

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

- IMAP STARTTLS and explicit plaintext support are wired.
- SMTP connection/authentication testing with `lettre` is wired.
- Normalize provider error mapping so bad credentials return a clear 400/401-ish
  response while network/server failures remain operational errors.

### 3. Read-Only Message Sync

- Folder refresh upserts folders and keeps stable ids.
- Per-folder sync state is persisted: UIDVALIDITY, UIDNEXT, lowest/highest
  scanned UID, sync timestamps, and last error.
- On UIDVALIDITY change, cached message summaries for that folder are
  invalidated before resync.
- Manual sync fetches message envelopes, flags, internal dates, and sizes in a
  bounded UID window. The default limit is 250 messages per folder per call,
  capped at 1000.
- Sync is latest-first. A folder without the new low/high cursor fetches the
  newest UID window first, then future calls prioritize newly arrived mail and
  backfill older UID windows toward UID 1.
- Next: add a scheduler/queue to run this strategy automatically.
- Fetch raw RFC822 bodies only when needed, or in bounded batches.
- Store raw messages and large decoded parts in Uncloud storage, not MongoDB.
- Keep MongoDB as searchable/listable metadata cache.

### 4. Message Read APIs

- Folder message listing uses cursor-based pagination over cached summaries.
- Basic message detail route exists and fetches/parses MIME bodies on demand.
- The UI progressively appends cached pages. When it reaches the cached edge
  for a folder whose UID window is not complete, it can trigger one bounded
  folder sync/backfill and then append any newly cached older messages.
- Store raw message fetches and parsed body output once the storage layout is
  decided.
- Add attachment metadata and download routes.
- Add an explicit remote image loading/proxy policy. Direct remote image URLs
  are stripped from sanitized HTML for now.

### 5. Mutations

- Mark read/unread, star/unstar, archive, move, and move-to-trash are wired as
  manual actions.
- Reflect remote flag/folder changes discovered outside Uncloud locally.
- Decide whether to support providers without `UID MOVE`, and if so, how to do
  it without unsafe expunge behavior.
- Add permanent delete semantics for Trash after the folder role model has been
  tested against real providers.
- Make mutations idempotent where possible and record failures clearly.
- Defer complex threading until list/detail sync is solid.

### 6. Sending

- Basic compose/send route using an identity and SMTP settings is wired.
- Sent-copy handling checks for provider-saved messages and appends to Sent when
  needed.
- Support HTML body after plain-text send has been tested with real providers.
- Add a user/provider setting for sent-copy policy once we know how common
  providers behave.
- Add reply/forward metadata handling.
- Add draft storage after basic send works.

### 7. UI

The first read-only UI shell exists. Continue improving it around real mailbox
data before adding write actions:

- Account setup, settings, deletion, and connection testing.
- Folder list, sync status, role labels, and per-folder settings.
- Read-only message list.
- Message reader with sanitized HTML and plain-text fallback.

Next UI work should focus on mailbox ergonomics, responsive navigation,
reader layout, and clearer sync/backfill state. Compose and mutations are
present as experimental workflows, but still need provider-specific hardening
and richer failure reporting.

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
