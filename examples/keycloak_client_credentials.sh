#!/usr/bin/env bash
# Keycloak client-credentials smoke test for the DocForge REST API.
# Development only: the realm import includes a known secret for docforge-api.
set -euo pipefail

DOCFORGE_URL="${DOCFORGE_URL:-http://localhost:8080}"
KEYCLOAK_URL="${KEYCLOAK_URL:-http://localhost:8081}"
REALM="${KEYCLOAK_REALM:-docforge}"
CLIENT_ID="${KEYCLOAK_CLIENT_ID:-docforge-api}"
CLIENT_SECRET="${KEYCLOAK_CLIENT_SECRET:-docforge-api-secret}"

echo "1/3 Requesting a service token from Keycloak..."
TOKEN_JSON=$(curl -fsS -X POST \
  "$KEYCLOAK_URL/realms/$REALM/protocol/openid-connect/token" \
  -H 'content-type: application/x-www-form-urlencoded' \
  --data-urlencode 'grant_type=client_credentials' \
  --data-urlencode "client_id=$CLIENT_ID" \
  --data-urlencode "client_secret=$CLIENT_SECRET" \
  --data-urlencode 'scope=convert')
ACCESS_TOKEN=$(printf '%s' "$TOKEN_JSON" | jq -r .access_token)
echo "    token: ${#ACCESS_TOKEN} chars"

echo "2/3 Calling the protected formats endpoint..."
curl -fsS "$DOCFORGE_URL/api/v1/formats" \
  -H "authorization: Bearer $ACCESS_TOKEN" \
  | jq '{count, first: .formats[0].id}'

echo "3/3 Converting a text file..."
INPUT_FILE="${INPUT_FILE:-$(mktemp)}"
if [ -z "${INPUT_FILE_EXPLICIT:-}" ]; then
  printf 'Keycloak + DocForge\n\nThis file was converted with a Keycloak service token.\n' > "$INPUT_FILE"
fi
OUTPUT="${OUTPUT:-keycloak-example.pdf}"
curl -fsS -X POST "$DOCFORGE_URL/api/v1/convert" \
  -H "authorization: Bearer $ACCESS_TOKEN" \
  -F "file=@$INPUT_FILE" \
  -F "target_format=pdf" \
  -o "$OUTPUT"
echo "    wrote $OUTPUT ($(wc -c < "$OUTPUT") bytes)"
echo
echo "Keycloak admin console: $KEYCLOAK_URL (admin / admin in dev)"
