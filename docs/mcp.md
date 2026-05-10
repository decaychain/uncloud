# MCP Endpoint — Design Document

**Status**: design. Phase 1 of the [OAuth + MCP rollout](oauth.md). Phase 0
shipped the OAuth surface; Phase 1 turns Uncloud into a usable remote MCP
server so Claude.ai's connector and `claude mcp add` can talk to it.

## Problem

Phase 0 stood up a standards-compliant OAuth 2.1 surface specifically so
that an MCP client (Claude.ai, MCP Inspector, future CLI tools) can
discover the server, register, and obtain a bearer token. None of that
infrastructure pays off until there is a `/mcp` endpoint on the other end
of the bearer.

The Model Context Protocol is the surface every Claude client speaks to
hand a model access to "tools" — functions the model can call to read or
modify external state. We expose three read-only tools in Phase 1; write
tools land in Phase 2 once `files:write` enforcement is in place on the
existing routes.

## Goals

- A working `/mcp` endpoint that MCP Inspector and Claude.ai can connect
  to with the OAuth bearer issued in Phase 0.
- Three tools (`list_files`, `read_file`, `search_files`) that wrap
  existing service-layer code — no new business logic.
- Pin to the MCP HTTP transport draft **2025-06-18** (Streamable HTTP).
  Re-evaluate on each MCP spec bump.
- Stay close enough to the protocol that any compliant MCP client works
  out of the box — no Uncloud-specific quirks.

## Non-goals (v1)

- **Write tools** (`upload_file`, `move_file`, `delete_file`). Deferred to
  Phase 2 alongside `files:write` / `files:delete` enforcement on the
  existing REST routes — same scopes, same checks, no point doing it twice.
- **MCP resources, prompts, sampling, roots**. Tools-only in v1. The
  `initialize` response advertises `tools` capability and nothing else.
- **MCP session management beyond the bare minimum**. We accept and echo
  the `Mcp-Session-Id` header so clients can pin to a single instance, but
  every request is fully resolvable from `(bearer, body)` alone — no
  per-session state on the server. Real sessions land if/when we add
  long-running tools (e.g. progress notifications, server-initiated
  events). See [Sessions](#sessions).
- **Backwards-compat with the older HTTP+SSE transport** (pre-2024-11-05).
  Streamable HTTP only.
- **Custom auth flows**. The bearer comes from `/oauth/token`. If a token
  lacks `files:read` the request is rejected at the JSON-RPC layer, not
  at the HTTP layer (see [Auth](#auth)).

## Transport

### Endpoint shape

```
POST /mcp        — JSON-RPC request, JSON or SSE response (per spec)
GET  /mcp        — open SSE stream for server-initiated messages (v1: 405)
DELETE /mcp      — explicit session terminate (v1: 405)
```

`/mcp` lives at the **root**, not under `/api/`. MCP convention places
the endpoint at a single well-known path; routing it under `/api/` would
fight discovery and add a translation step every Inspector demo.

### Content type

- Request body: `application/json` (single JSON-RPC request per call;
  batches deferred — see [Risks](#risks)).
- Response: `application/json` for tool-call responses small enough to
  return synchronously. We do **not** open an SSE stream for tool calls
  in v1 — every tool we ship completes in well under a second against
  Mongo and Meilisearch. The SSE response shape is reserved for Phase 2
  if a long-running tool needs progress notifications.

### Headers

- `Authorization: Bearer <oauth-access-token>` — required.
- `MCP-Protocol-Version: 2025-06-18` — required after `initialize` per
  spec. We accept any value during `initialize` and lock to the
  negotiated version for subsequent calls on the same session.
- `Mcp-Session-Id: <opaque>` — optional. We mint one on `initialize`
  and echo it back; clients send it on subsequent requests. We do **not**
  use it for state lookup in v1 (see [Sessions](#sessions)).

## Sessions

The MCP spec allows servers to be either stateless or session-bound.
Stateless is dramatically simpler and fits a tools-only server fine.

We mint a `Mcp-Session-Id` on `initialize` (a fresh ULID) and echo it on
all responses. The server does **not** key any state off the session id
in v1 — every JSON-RPC call carries its own bearer, and the bearer fully
identifies the user. The session id exists only so that Inspector and
Claude.ai see a stable identity for log correlation.

If Phase 2 adds a long-running tool (e.g. `wait_for_upload`), session
state moves into a `mcp_sessions` collection keyed by the same id.
The handle is reserved now so we don't have to retrofit headers later.

## Auth

OAuth bearer plumbed in Phase 0. The middleware already attaches
`AuthUser` and `Scopes(Option<Vec<String>>)` to the request.

Scope check happens **inside the JSON-RPC handler**, not at the route
level, because:

1. `initialize`, `ping`, `tools/list` should succeed for any authenticated
   bearer — the client needs to discover what it can do before knowing
   whether it has the right scopes for any specific call.
2. `tools/call` checks the scope required by the named tool. All three v1
   tools require `files:read`. Phase 2 tools will require `files:write`
   or `files:delete`.

Failures map to JSON-RPC errors (not HTTP 4xx) so the client sees a
structured response:

| Cause | JSON-RPC code | Message |
|---|---|---|
| Missing/invalid bearer | -32001 | "Authentication required" (HTTP 401, also serves the JSON-RPC body) |
| Insufficient scope | -32002 | "Scope `files:read` required" |
| Unknown tool | -32601 | "Method not found" (per JSON-RPC spec) |
| Bad params | -32602 | "Invalid params" |

The HTTP response stays 200 for everything past authentication; clients
parse JSON-RPC `error` objects regardless.

## JSON-RPC methods

### `initialize`

Request:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2025-06-18",
    "capabilities": { "roots": {} },
    "clientInfo": { "name": "Claude.ai", "version": "..." }
  }
}
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": "2025-06-18",
    "capabilities": { "tools": {} },
    "serverInfo": { "name": "uncloud", "version": "<crate version>" },
    "instructions": "Uncloud personal cloud storage. Read-only file tools."
  }
}
```

`Mcp-Session-Id` set on the response. We accept whatever
`protocolVersion` the client offers; if it isn't `2025-06-18` we still
return ours and the client decides whether to continue.

### `ping`

Request: `{ "jsonrpc": "2.0", "id": N, "method": "ping" }`. Response:
`{ "jsonrpc": "2.0", "id": N, "result": {} }`. Used by Inspector to test
connectivity.

### `tools/list`

Returns the three tools below. Static, no params.

### `tools/call`

Request:

```json
{
  "jsonrpc": "2.0",
  "id": N,
  "method": "tools/call",
  "params": {
    "name": "list_files",
    "arguments": { "folder_id": "..." }
  }
}
```

Response (success):

```json
{
  "jsonrpc": "2.0",
  "id": N,
  "result": {
    "content": [
      { "type": "text", "text": "<JSON-encoded result>" }
    ],
    "isError": false
  }
}
```

We return tool output as a single `text` block containing pretty-printed
JSON. MCP also supports structured `content` parts, but the universal
shape today is "text block with JSON inside" — Claude.ai parses it
fine, Inspector renders it readably, and we don't have to invent a
schema language for output (the input schema is enough to drive the
model's call site).

On error: `isError: true`, `content[0].text` carries a human message.

## Tools

All three require `files:read`. Output is a JSON-encoded string in a
`text` content block (see above).

### `list_files`

Wraps `routes::folders::list_folders` + `routes::files::list_files`.

Input schema:

```json
{
  "type": "object",
  "properties": {
    "folder_id": {
      "type": "string",
      "description": "Folder ObjectId hex. Omit or empty string for the root folder."
    },
    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 50 }
  }
}
```

Output (JSON in the text block):

```json
{
  "folder": { "id": "...", "name": "..." },
  "folders": [{ "id": "...", "name": "...", "size_bytes": 0 }],
  "files":   [{ "id": "...", "name": "...", "mime_type": "...", "size_bytes": 0 }],
  "next_cursor": null
}
```

Cursor field is reserved for pagination — v1 returns up to `limit`
entries and a null cursor. Pagination lands when somebody hits the limit
in practice.

### `read_file`

Wraps `routes::files::download_file`, but capped and text-only in v1.

Input:

```json
{
  "type": "object",
  "required": ["file_id"],
  "properties": {
    "file_id": { "type": "string" },
    "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "default": 65536 }
  }
}
```

Behaviour:

- Reads up to `max_bytes` from the storage backend.
- If `mime_type` starts with `text/` or matches the text-extract
  allowlist (`application/json`, `application/xml`,
  `application/javascript`, `application/pdf`), return the body as
  `text`.
- For `application/pdf`, return the cached `metadata.content_text` from
  the text-extract pipeline if present (already truncated to 1 MB at
  ingest); otherwise extract on the fly. **Do not** stream raw PDF
  bytes.
- For other binary types, return an error. Phase 2 may add a
  `read_file_binary` that returns `image` content blocks; not now.

The 1 MB ceiling matches the existing text-extract cap, so model
context stays bounded.

### `search_files`

Wraps `services::search::SearchService::search`.

Input:

```json
{
  "type": "object",
  "required": ["query"],
  "properties": {
    "query": { "type": "string", "minLength": 1 },
    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 }
  }
}
```

Output: array of hits as Meilisearch returns them — `{ id, name,
mime_type, size_bytes, parent_id, snippet }`. Owner filter is enforced
server-side as it already is on `/api/search`.

If Meilisearch is disabled in config, the tool returns an empty result
plus a `disabled: true` field in the JSON. We don't synthesise a
substring scan over Mongo as a fallback — the search service already
makes that choice once.

## Storage

None. Phase 1 is read-only and stateless.

A future `mcp_sessions` collection (see [Sessions](#sessions)) is
deferred. The session id is opaque to the server today, so adding
storage later is a back-compat-safe schema change.

## Discovery wiring

Already in place from Phase 0:

- `GET /.well-known/oauth-protected-resource` lists `resource: "<base>/mcp"`.
- `GET /.well-known/oauth-authorization-server` lives under the same
  base URL.

MCP Inspector (and Claude.ai) walk these to find the authorize endpoint
and exchange a token. No changes needed in Phase 1 — the discovery
metadata already advertises the resource path; we just have to make the
path work.

## Implementation plan

Single PR, a handful of files:

1. `crates/uncloud-server/src/mcp/mod.rs` — module entrypoint.
2. `crates/uncloud-server/src/mcp/jsonrpc.rs` — request/response types,
   error codes, batch-rejection (out of scope for v1, return -32600).
3. `crates/uncloud-server/src/mcp/handler.rs` — `POST /mcp` Axum
   handler. Dispatches `initialize` / `ping` / `tools/list` / `tools/call`.
4. `crates/uncloud-server/src/mcp/tools/mod.rs` — `Tool` trait
   (`name`, `description`, `input_schema`, `required_scope`, `call`).
5. `crates/uncloud-server/src/mcp/tools/{list_files,read_file,search_files}.rs`
   — three tool impls. Each one is a thin adapter that constructs the
   same query the corresponding REST handler runs (or extracts the
   shared inner function — preferred where the existing handler is
   small enough to refactor).
6. Route wiring in `routes/mod.rs`: mount `/mcp` outside the existing
   `/api` group, with the auth middleware applied so the bearer
   resolves and `Scopes` is on the request.
7. `serde_json::Value` for tool arguments (no per-tool typed structs in
   v1 — keeps the dispatcher uniform). Each tool validates its own
   shape and returns a JSON-RPC `-32602` on bad input.

Where the existing REST handlers contain logic that ought to be reused
(e.g. the gallery / list_files joining), refactor the shared piece
into a function the route handler calls and the MCP tool calls — don't
duplicate.

## Testing

Three layers, mirroring Phase 0:

1. **Rust integration tests** (`crates/uncloud-server/tests/mcp.rs`):
   - `initialize` returns `protocolVersion: 2025-06-18` and the tools
     capability.
   - `tools/list` returns three tools with valid JSON schemas.
   - `tools/call` with `list_files`, `read_file`, `search_files`
     against a seeded user works.
   - Bearer with no `files:read` scope receives `-32002`.
   - Missing bearer receives 401.
   - Unknown tool name receives `-32601`.
   - PDF `read_file` returns the cached `content_text` when present.

2. **MCP Inspector** — `npx @modelcontextprotocol/inspector` against
   `http://localhost:8080/mcp` with the OAuth flow from Phase 0's smoke
   script. Exercises the same code path Claude.ai uses; if Inspector
   works, Claude.ai's connector works.

3. **Manual Claude.ai connect** — once Inspector passes, add Uncloud as
   a custom connector. This is the actual product target; Inspector
   is the cheap iteration loop.

No `scripts/mcp-smoke.sh` — Inspector is the equivalent and is already
the recommended tool by the spec maintainers.

## Phasing

- **Phase 1** (this PR): everything above — read-only tools, stateless
  Streamable HTTP.
- **Phase 2**: write tools (`upload_file`, `move_file`, `delete_file`,
  `create_folder`, `restore_from_trash`) gated on `files:write` /
  `files:delete`. Adds scope enforcement to the existing REST routes
  for parity.
- **Phase 3** (optional): MCP `resources` exposing folders as a tree,
  long-running tools with progress notifications via SSE, and
  `mcp_sessions` storage.

## Risks

- **Spec drift in MCP HTTP**. The transport spec changed shape between
  drafts (HTTP+SSE → Streamable HTTP). We pin to 2025-06-18 and
  re-evaluate on each spec bump; the `protocolVersion` field in
  `initialize` makes a server-side switch easy if we ever support more
  than one.
- **JSON-RPC batches**. The MCP spec requires servers to handle batch
  requests on `POST /mcp`. We reject batches with `-32600` ("Invalid
  Request") in v1 — Inspector and Claude.ai don't batch, and the extra
  branch isn't worth carrying until something needs it. Document the
  limitation and revisit if a real client asks for it.
- **Tool input validation**. We validate via `serde_json` against the
  declared schema by hand (size caps, required fields, type checks).
  A full JSON Schema validator pulls a heavy dep tree for marginal
  benefit on three tools. Re-evaluate if the tool count grows past
  ~10.
- **Read-amplification on `read_file` for PDFs without cached text**.
  The fallback path runs the text-extract subprocess synchronously;
  worst case is the existing 60s timeout. Acceptable for v1.
- **No CORS on `/mcp`**. The MCP transport is server-to-server (Claude
  backend, not a browser), so we don't add CORS headers. Re-add if
  somebody builds a browser-based MCP client.
