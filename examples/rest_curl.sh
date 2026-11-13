#!/usr/bin/env bash
# Complete OAuth 2.1 authorization-code + PKCE flow and a REST conversion.
# Requires: curl, openssl, jq. Development/demo usage; the login POST is
# automated for convenience. A real client should use a browser.
set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:8080}"
BASE_URL="${BASE_URL%/}"
USERNAME="${AUTH_USERNAME:-demo}"
PASSWORD="${AUTH_PASSWORD:-demo}"
TARGET="${TARGET:-pdf}"
REDIRECT="${REDIRECT:-http://127.0.0.1:9876/callback}"

b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }

echo "1/5 Registering a public PKCE client..."
CLIENT_JSON=$(curl -fsS -X POST "$BASE_URL/oauth/register" \
  -H 'content-type: application/json' \
  -d "{\"client_name\":\"DocForge bash example\",\"redirect_uris\":[\"$REDIRECT\"],\"grant_types\":[\"authorization_code\",\"refresh_token\"],\"response_types\":[\"code\"],\"token_endpoint_auth_method\":\"none\",\"scope\":\"convert mcp offline_access\"}")
CLIENT_ID=$(printf '%s' "$CLIENT_JSON" | jq -r .client_id)

VERIFIER=$(openssl rand -base64 48 | tr -d '\n' | tr '+/' '-_' | tr -d '=')
CHALLENGE=$(printf '%s' "$VERIFIER" | openssl dgst -sha256 -binary | b64url)

echo "2/5 Authorizing (demo credentials)..."
LOCATION=$(curl -sS -o /dev/null -D - -X POST "$BASE_URL/oauth/authorize" \
  -H 'content-type: application/x-www-form-urlencoded' \
  --data-urlencode 'response_type=code' \
  --data-urlencode "client_id=$CLIENT_ID" \
  --data-urlencode "redirect_uri=$REDIRECT" \
  --data-urlencode 'scope=convert offline_access' \
  --data-urlencode 'state=example' \
  --data-urlencode "code_challenge=$CHALLENGE" \
  --data-urlencode 'code_challenge_method=S256' \
  --data-urlencode "resource=$BASE_URL/api" \
  --data-urlencode "username=$USERNAME" \
  --data-urlencode "password=$PASSWORD" \
  | awk 'BEGIN{IGNORECASE=1} /^location:/{sub(/\r$/,""); print substr($0,11)}')
CODE=$(printf '%s' "$LOCATION" | sed -n 's/.*[?&]code=\([^&]*\).*/\1/p')
[ -n "$CODE" ] || { echo "Authorization failed: $LOCATION" >&2; exit 1; }

echo "3/5 Exchanging the code for tokens..."
TOKEN_JSON=$(curl -fsS -X POST "$BASE_URL/oauth/token" \
  -H 'content-type: application/x-www-form-urlencoded' \
  --data-urlencode 'grant_type=authorization_code' \
  --data-urlencode "code=$CODE" \
  --data-urlencode "redirect_uri=$REDIRECT" \
  --data-urlencode "client_id=$CLIENT_ID" \
  --data-urlencode "code_verifier=$VERIFIER" \
  --data-urlencode "resource=$BASE_URL/api")
ACCESS_TOKEN=$(printf '%s' "$TOKEN_JSON" | jq -r .access_token)
SCOPE=$(printf '%s' "$TOKEN_JSON" | jq -r .scope)
echo "    access token: ${#ACCESS_TOKEN} chars, scope=$SCOPE"

echo "4/5 Calling a protected endpoint..."
curl -fsS "$BASE_URL/api/v1/formats" -H "authorization: Bearer $ACCESS_TOKEN" \
  | jq '{count, first: .formats[0].id}'

echo "5/5 Converting..."
if [ -z "${INPUT_FILE:-}" ]; then
  INPUT_FILE="$(mktemp)"
  printf 'DocForge example\n\nConverted through the OAuth 2.1 REST API.\n' > "$INPUT_FILE"
fi
OUTPUT="${OUTPUT:-docforge-example.$TARGET}"
curl -fsS -X POST "$BASE_URL/api/v1/convert" \
  -H "authorization: Bearer $ACCESS_TOKEN" \
  -F "file=@$INPUT_FILE" \
  -F "target_format=$TARGET" \
  -o "$OUTPUT"
echo "    wrote $OUTPUT ($(wc -c < "$OUTPUT") bytes)"
