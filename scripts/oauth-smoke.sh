#!/usr/bin/env bash
# OAuth 2.1 + PKCE end-to-end smoke test.
#
# Walks the full flow against a local Uncloud server: dynamic client
# registration → authorize (browser consent) → token exchange → call
# /api/v1/auth/me with the issued access token.
#
# Requires: curl, jq, python3, openssl, a browser, and an authenticated
# session (the consent step is rendered in the browser, so the user must
# already be logged in to the server's web UI in that browser).
#
# Usage:
#   SERVER_URL=http://localhost:8080 ./scripts/oauth-smoke.sh

set -euo pipefail

SERVER_URL="${SERVER_URL:-http://localhost:8080}"
CALLBACK_PORT="${CALLBACK_PORT:-8765}"
REDIRECT_URI="http://localhost:${CALLBACK_PORT}/callback"
SCOPE="${SCOPE:-files:read}"

echo "Server:        $SERVER_URL"
echo "Callback URI:  $REDIRECT_URI"
echo "Scope:         $SCOPE"
echo

# ---------------------------------------------------------------------------
# 1. Register an OAuth client
# ---------------------------------------------------------------------------
echo "[1/5] Registering OAuth client..."
REGISTER_BODY=$(jq -n \
  --arg name "oauth-smoke.sh" \
  --arg redir "$REDIRECT_URI" \
  --arg scope "$SCOPE" \
  '{client_name: $name, redirect_uris: [$redir], token_endpoint_auth_method: "none", scope: $scope}')

REGISTER_RESPONSE=$(curl -sS -X POST "${SERVER_URL}/oauth/register" \
  -H "Content-Type: application/json" \
  -d "$REGISTER_BODY")

CLIENT_ID=$(echo "$REGISTER_RESPONSE" | jq -er .client_id) \
  || { echo "  registration failed: $REGISTER_RESPONSE"; exit 1; }
echo "  client_id: $CLIENT_ID"

# ---------------------------------------------------------------------------
# 2. Generate PKCE verifier + S256 challenge
# ---------------------------------------------------------------------------
echo "[2/5] Generating PKCE verifier..."
# 32 random bytes → base64url (no padding) → 43 chars verifier
VERIFIER=$(head -c 32 /dev/urandom | openssl base64 -A | tr '/+' '_-' | tr -d '=')
CHALLENGE=$(printf '%s' "$VERIFIER" | openssl dgst -sha256 -binary \
  | openssl base64 -A | tr '/+' '_-' | tr -d '=')
STATE=$(head -c 16 /dev/urandom | openssl base64 -A | tr '/+' '_-' | tr -d '=')
echo "  verifier:  ${VERIFIER:0:24}..."
echo "  challenge: ${CHALLENGE:0:24}..."

# ---------------------------------------------------------------------------
# 3. Build authorize URL, open browser, await callback
# ---------------------------------------------------------------------------
urlenc() { jq -nr --arg s "$1" '$s|@uri'; }

AUTH_URL="${SERVER_URL}/oauth/authorize"
AUTH_URL+="?client_id=$(urlenc "$CLIENT_ID")"
AUTH_URL+="&redirect_uri=$(urlenc "$REDIRECT_URI")"
AUTH_URL+="&response_type=code"
AUTH_URL+="&scope=$(urlenc "$SCOPE")"
AUTH_URL+="&state=$(urlenc "$STATE")"
AUTH_URL+="&code_challenge=${CHALLENGE}"
AUTH_URL+="&code_challenge_method=S256"

echo "[3/5] Opening browser for consent..."
echo "  $AUTH_URL"
if command -v xdg-open >/dev/null; then
  xdg-open "$AUTH_URL" >/dev/null 2>&1 &
elif command -v open >/dev/null; then
  open "$AUTH_URL" >/dev/null 2>&1 &
fi

echo "  listening on port $CALLBACK_PORT for the redirect..."
CALLBACK_PATH=$(python3 - "$CALLBACK_PORT" <<'PY'
import http.server, socketserver, sys
port = int(sys.argv[1])
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a, **k): pass
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/html')
        self.end_headers()
        self.wfile.write(b'<html><body><h2>Authorization received. You can close this tab.</h2></body></html>')
        print(self.path, flush=True)
        sys.exit(0)
with socketserver.TCPServer(('127.0.0.1', port), H) as srv:
    srv.handle_request()
PY
)

QS="${CALLBACK_PATH#*\?}"
CODE=""
RECEIVED_STATE=""
ERROR_PARAM=""
IFS='&' read -ra PAIRS <<< "$QS"
for kv in "${PAIRS[@]}"; do
  case "$kv" in
    code=*)  CODE="${kv#code=}";;
    state=*) RECEIVED_STATE="${kv#state=}";;
    error=*) ERROR_PARAM="${kv#error=}";;
  esac
done

if [ -n "$ERROR_PARAM" ]; then
  echo "  authorization denied or failed: $ERROR_PARAM"
  exit 1
fi
if [ -z "$CODE" ]; then
  echo "  no code in callback (qs=$QS)"
  exit 1
fi
if [ "$RECEIVED_STATE" != "$STATE" ]; then
  echo "  state mismatch (expected=$STATE got=$RECEIVED_STATE)"
  exit 1
fi
echo "  code: ${CODE:0:24}..."

# ---------------------------------------------------------------------------
# 4. Exchange code for access token
# ---------------------------------------------------------------------------
echo "[4/5] Exchanging code for access token..."
TOKEN_RESPONSE=$(curl -sS -X POST "${SERVER_URL}/oauth/token" \
  --data-urlencode "grant_type=authorization_code" \
  --data-urlencode "code=${CODE}" \
  --data-urlencode "client_id=${CLIENT_ID}" \
  --data-urlencode "redirect_uri=${REDIRECT_URI}" \
  --data-urlencode "code_verifier=${VERIFIER}")

ACCESS_TOKEN=$(echo "$TOKEN_RESPONSE" | jq -er .access_token) \
  || { echo "  token exchange failed: $TOKEN_RESPONSE"; exit 1; }
SCOPE_GRANTED=$(echo "$TOKEN_RESPONSE" | jq -r .scope)
EXPIRES_IN=$(echo "$TOKEN_RESPONSE" | jq -r .expires_in)
REFRESH=$(echo "$TOKEN_RESPONSE" | jq -r .refresh_token)
echo "  access_token: ${ACCESS_TOKEN:0:24}..."
echo "  refresh_token: ${REFRESH:0:24}..."
echo "  scope: $SCOPE_GRANTED"
echo "  expires_in: ${EXPIRES_IN}s"

# ---------------------------------------------------------------------------
# 5. Use the access token to call /api/v1/auth/me
# ---------------------------------------------------------------------------
echo "[5/5] Calling /api/v1/auth/me with the issued bearer..."
ME=$(curl -sS "${SERVER_URL}/api/v1/auth/me" \
  -H "Authorization: Bearer ${ACCESS_TOKEN}")
echo "$ME" | jq .

echo
echo "OAuth smoke test passed."
