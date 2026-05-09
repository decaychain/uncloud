# OAuth 2.1 Authorization Server — Design Document

**Status**: design + Phase 0 implementation. Captures the model so subsequent
phases (MCP integration, scope enforcement everywhere, app-platform migration)
can land incrementally without re-deriving it.

## Problem

Two pressures converged:

1. **MCP**: Claude.ai (and Claude Code's `claude mcp add`) connect to remote
   MCP servers via OAuth 2.1 with PKCE. The MCP HTTP spec mandates
   discovery (RFC 8414, RFC 9728) and dynamic client registration
   (RFC 7591). Bearer tokens alone don't satisfy the connector flow.
2. **Scoped third-party access**: today's personal access tokens
   (`/api/v1/auth/tokens`) are all-or-nothing — every token has full account
   access. Any third-party tool a user wants to grant access to (CLI
   utilities, mobile apps, future federation) needs scopes and per-app
   revocation.

A standards-compliant OAuth 2.1 surface solves both — MCP is just one of
many clients that consumes it.

## Goals

- Make Uncloud a usable remote MCP server for Claude.ai / `claude mcp add`
  in Phase 1, by getting the OAuth surface right in Phase 0.
- Add scoped access tokens without breaking the existing PAT model.
- Stay close enough to the spec that any compliant OAuth 2.1 client works
  out of the box — no Uncloud-specific quirks.

## Non-goals (v1)

- **Confidential clients** (clients that can hold a secret). Public clients
  + PKCE only. Easy to add later.
- **Migrating the `/apps` reverse-proxy platform off session-sharing**. The
  in-server iframe apps are a different concept; they keep their own
  registration mechanism for now. A future phase can move them onto OAuth.
- **JWTs**. Access tokens are opaque random strings, hashed at rest, looked
  up by hash on each request — same shape as today's PATs. JWTs are a
  rabbit hole (key rotation, JWK URLs, `kid` headers); not worth the
  complexity until we have a real driver.
- **Scope enforcement on every existing route**. Phase 0 attaches scopes to
  the request when present; routes that need to gate on scopes opt in.
  Legacy PATs (no scopes set) keep working unchanged.

## Auth model

Three kinds of bearer credentials, all resolved by the same
`auth_middleware` and stored in the same `api_tokens` collection
(extended with optional fields):

| Kind | `client_id` | `scopes` | `expires_at` | `refresh_token_hash` | Issued by |
|---|---|---|---|---|---|
| Session token | None | None | session expiry | None | Login |
| Personal access token (legacy) | None | None | None | None | `POST /api/v1/auth/tokens` |
| OAuth access token | Some | Some(Vec) | Some | Some | `POST /oauth/token` |

A request hits a protected route. The middleware:

1. Looks up the bearer.
2. If the token has `expires_at` and it's past, rejects.
3. Attaches `(AuthUser, Option<Vec<Scope>>)` to the request extensions.
   Sessions and legacy PATs attach `None` (= "no scope filter, full access").
4. Routes that opt in to scope checking (Phase 1+ work) extract the scopes
   and gate behaviour. Routes that don't (everything today) ignore them.

This keeps the blast radius of Phase 0 tiny: the entire existing API
surface is unchanged for sessions and PATs.

## Scopes (initial set)

Three scopes in v1, kept deliberately small:

- `files:read` — list / search / read files, folders, gallery, music; download.
- `files:write` — create folders, upload, rename, move, copy, restore from
  trash; update metadata.
- `files:delete` — soft-delete (move to trash) and permanent delete.

A token with no scopes set (legacy PAT) bypasses all scope checks.
A token issued via OAuth must have at least one scope.

Future scopes (deferred, but the namespace is reserved):
`tasks:read`, `tasks:write`, `passwords:read`, `passwords:write`, `admin`.

The `admin` scope deliberately has no v1 representation — admin
operations stay session-only until we have a driver for granting them
externally.

## Endpoints

### Discovery (public)

- `GET /.well-known/oauth-authorization-server` — RFC 8414 metadata.
- `GET /.well-known/oauth-protected-resource` — RFC 9728 metadata.

Both return JSON describing the server's capabilities, supported scopes,
and where to find the other endpoints. MCP clients fetch these to bootstrap.

### Client registration (public)

- `POST /oauth/register` — RFC 7591 dynamic client registration.

Body:

```json
{
  "client_name": "Claude.ai (mcp)",
  "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"],
  "token_endpoint_auth_method": "none",
  "grant_types": ["authorization_code", "refresh_token"],
  "response_types": ["code"],
  "scope": "files:read"
}
```

Returns `{ client_id, client_id_issued_at, ... }`. Public clients only
(no `client_secret` issued). Rate-limited.

### Authorization (browser-driven)

- `GET /oauth/authorize?client_id=...&redirect_uri=...&response_type=code&scope=...&state=...&code_challenge=...&code_challenge_method=S256`

Validates the request, then redirects to the Dioxus consent page at
`/oauth/authorize` (frontend route, same path) with the validated params
preserved in the query string. The consent page renders:

> **Claude.ai** wants to access your Uncloud account.
> It will be able to: read your files and folders.
> [Allow] [Deny]

- `POST /oauth/authorize` — consent submit. Mints an authorization code
  bound to `(client_id, user_id, scopes, redirect_uri, code_challenge)`,
  stores it hashed in `oauth_authorization_codes` with a 10-minute expiry,
  responds 302 to `redirect_uri?code=...&state=...` (or
  `?error=access_denied&state=...` on deny).

The user must be authenticated via session cookie. Unauthenticated
requests redirect to `/login?next=/oauth/authorize?...` and bounce back
after login.

### Token (back-channel)

- `POST /oauth/token` (form-encoded, per spec)

Two grant types:

**`grant_type=authorization_code`**: body has
`code, redirect_uri, client_id, code_verifier`. Validates the code (exists,
unused, not expired, matches client + redirect_uri), verifies PKCE
(`SHA256(code_verifier) == code_challenge`), marks the code consumed,
mints `(access_token, refresh_token)`, stores them hashed in `api_tokens`.

Response:

```json
{
  "access_token": "<opaque>",
  "token_type": "Bearer",
  "expires_in": 3600,
  "refresh_token": "<opaque>",
  "scope": "files:read"
}
```

**`grant_type=refresh_token`**: body has `refresh_token, client_id`.
Validates, rotates (issues a new access + refresh pair, invalidates the
old refresh), returns same shape as above.

### Revocation

- `POST /oauth/revoke` — RFC 7009. Body: `token, token_type_hint`.
  Deletes the matching `api_tokens` row.

### Connected-apps management (authenticated)

- `GET /api/v1/oauth/clients` — lists OAuth clients with active tokens for
  the current user (group `api_tokens` by `client_id`, join with
  `oauth_clients` for the human-readable name).
- `DELETE /api/v1/oauth/clients/{client_id}` — revokes all tokens issued
  to that client for the current user.

Surfaced as a "Connected apps" tab in the existing Settings page.

## Storage

### `oauth_clients`

```rust
pub struct OAuthClient {
    pub id: ObjectId,
    pub client_id: String,         // public identifier; ULID-like
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub allowed_scopes: Vec<String>,
    pub dynamically_registered: bool,
    pub created_at: DateTime<Utc>,
}
```

`client_secret` is reserved for a future confidential-client phase.

### `oauth_authorization_codes`

```rust
pub struct OAuthAuthorizationCode {
    pub id: ObjectId,
    pub code_hash: String,         // SHA-256
    pub client_id: String,
    pub user_id: ObjectId,
    pub scopes: Vec<String>,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String, // "S256" only
    pub expires_at: DateTime<Utc>,
    pub consumed: bool,
}
```

TTL index on `expires_at` for automatic cleanup.

### `api_tokens` (extended)

New optional fields, all back-compat:

```rust
pub struct ApiToken {
    // existing
    pub id: ObjectId,
    pub user_id: ObjectId,
    pub name: String,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    // new — None for legacy PATs
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    #[serde(default, with = "crate::models::opt_dt")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub refresh_token_hash: Option<String>,
}
```

OAuth-issued tokens have `client_id`/`scopes`/`expires_at`/`refresh_token_hash`
populated. The `name` field is set to the human-readable client name for
the connected-apps UI.

## PKCE

S256 only. Plain method is rejected (the spec discourages it; no v1
client needs it).

```
code_challenge = base64url_no_pad(sha256(code_verifier))
```

`code_verifier` is 43-128 unreserved-chars; `code_challenge` is the same.

## Smart defaults

- Authorization code TTL: 10 minutes.
- Access token TTL: 1 hour.
- Refresh token TTL: 30 days, rotates on every use.
- Dynamic registration: open for now (rate-limited at the proxy layer).

All configurable via `config.yaml` if the defaults turn out wrong, but
not exposed as settings until there's evidence they need to be.

## Frontend

### Routes

- `/oauth/authorize` — Dioxus consent page. Reads params from the URL,
  fetches `GET /api/v1/oauth/clients/lookup?client_id=...` for the
  human-readable name, posts the consent decision back to
  `/oauth/authorize`.

### Settings

- New tab "Connected apps" — lists OAuth clients with revoke action. Sits
  next to the existing "API tokens" tab.

## Testing

Three layers — see [Phase-0 testing](#phase-0-testing) below.

### Phase-0 testing

1. **Rust integration tests** (`crates/uncloud-server/tests/oauth.rs`):
   - Discovery endpoints return the expected fields.
   - Dynamic registration creates a client and returns `client_id`.
   - `/oauth/authorize` 302s with code on consent.
   - `/oauth/token` exchange with valid PKCE works; bad verifier rejects;
     expired code rejects; mismatched `redirect_uri` rejects.
   - Refresh rotates the refresh token (old one no longer works).
   - Revoke invalidates the access token.
   - An OAuth-issued token authenticates a request; legacy PATs still work.

2. **`scripts/oauth-smoke.sh`** — end-to-end script. Registers a client,
   generates PKCE, opens the authorize URL in the browser, captures the
   redirect on a local listener, exchanges for an access token, calls
   `/api/v1/auth/me`. Committed as the manual repro.

3. **MCP Inspector** — point `@modelcontextprotocol/inspector` at our
   discovery URL. Even with no `/mcp` endpoint in Phase 0, Inspector walks
   authorize → token and surfaces the issued token. Same code path
   Claude.ai uses; if Inspector works, Claude.ai works in Phase 1.

## Phasing

- **Phase 0** (this PR): everything above — auth surface, no MCP yet, no
  scope enforcement on existing routes.
- **Phase 1**: `/mcp` Streamable-HTTP endpoint with read-only tools
  (`list_files`, `read_file`, `search_files`), gated on `files:read`.
- **Phase 2**: write tools and `files:write` / `files:delete` enforcement
  on the relevant existing routes.
- **Phase 3** (optional): migrate the `/apps` reverse-proxy platform to
  OAuth client credentials.

## Risks

- **Spec drift in MCP auth**. The MCP HTTP transport spec is still
  evolving (the auth annex changed between 2025-03-26 and 2025-06-18).
  We pin to the 2025-06-18 draft and revisit on each MCP spec bump.
- **Public-client + PKCE means "anyone who can intercept the redirect can
  exchange the code"**. Mitigated by HTTPS in production, and by the
  `redirect_uri` allowlist enforced at code-mint time.
- **Open dynamic registration is a small DoS surface**. Bounded by:
  registration rate-limit (TODO: add at the proxy / a per-IP throttle in
  the handler), and the cleanup job that removes clients with no active
  tokens older than N days (deferred to Phase 1).
- **`api_tokens` schema growth**. Extending an existing collection means
  every new optional field has to be back-compat. So far the additions
  are all `Option<T>` and `#[serde(default)]`, so old documents
  deserialise fine.
