# Mail Client Foundation

Experimental design for adding a first-party email client to Uncloud. The goal
is a mail client that connects to external providers over IMAP/SMTP, not a mail
server.

## Scope

The foundation supports:

- Multiple mail accounts per Uncloud user.
- Multiple send identities per account.
- IMAP folders and subfolders, stored by remote path plus hierarchy delimiter.
- Message and attachment metadata models for a cached mailbox.
- A provider boundary backed initially by `async-imap`.
- SMTP/MIME/sanitization dependencies selected for later send and render work:
  `lettre`, `mail-parser`, and `ammonia`.

The first implementation deliberately does not persist provider passwords or
OAuth refresh tokens. Endpoints that need provider access accept the password
transiently. Persistent background sync needs encrypted server-side secrets
before it is enabled.

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
- `POST /mail/accounts/{id}/test-imap`
- `GET /mail/accounts/{account_id}/folders`
- `POST /mail/accounts/{account_id}/folders/refresh`
- `GET /mail/identities`
- `POST /mail/identities`
- `PUT /mail/identities/{id}`
- `DELETE /mail/identities/{id}`

`test-imap` and `folders/refresh` currently require implicit TLS IMAP. STARTTLS
and plaintext ports are represented in the data model but not wired yet.

## Next Work

- Add encrypted credential storage before any background sync.
- Add SMTP connection testing with `lettre`.
- Implement mailbox sync using UIDVALIDITY, UIDNEXT, and per-folder high-water
  marks.
- Parse RFC822 messages with `mail-parser`, sanitize HTML with `ammonia`, and
  store raw bodies/attachments in Uncloud storage instead of MongoDB.
- Add message list/detail routes.
- Build the Dioxus mail UI once the backend cache can serve real messages.
