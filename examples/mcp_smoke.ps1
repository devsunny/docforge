# MCP smoke test (development)
#
# This uses a confidential client + client_credentials to test the MCP transport
# without a browser. Real MCP clients should use the discovery + DCR + PKCE flow
# described in docs/MCP.md.

$ErrorActionPreference = "Stop"
$BaseUrl = "http://127.0.0.1:8080"

$client = Invoke-RestMethod -Method Post -Uri "$BaseUrl/oauth/register" `
    -ContentType "application/json" -Body (@{
        client_name                = "MCP smoke"
        redirect_uris              = @("http://127.0.0.1:9876/callback")
        grant_types                = @("authorization_code", "client_credentials")
        token_endpoint_auth_method = "client_secret_basic"
        scope                      = "mcp"
    } | ConvertTo-Json -Depth 5)

$token = Invoke-RestMethod -Method Post -Uri "$BaseUrl/oauth/token" `
    -ContentType "application/x-www-form-urlencoded" -Body @{
        grant_type    = "client_credentials"
        client_id     = $client.client_id
        client_secret = $client.client_secret
        scope         = "mcp"
        resource      = "$BaseUrl/mcp"
    }

$temp = Join-Path ([System.IO.Path]::GetTempPath()) "docforge-mcp"
New-Item -ItemType Directory -Force $temp | Out-Null
function Invoke-Mcp($body, $session) {
    $file = Join-Path $temp "request.json"
    $body | ConvertTo-Json -Depth 10 -Compress | Set-Content -NoNewline $file
    $curlArgs = @(
        "-s", "-D", (Join-Path $temp "headers.txt"), "-X", "POST", "$BaseUrl/mcp",
        "-H", "Authorization: Bearer $($token.access_token)",
        "-H", "Content-Type: application/json",
        "-H", "MCP-Protocol-Version: 2025-06-18"
    )
    if ($session) { $curlArgs += @("-H", "Mcp-Session-Id: $session") }
    $curlArgs += @("--data-binary", "@$file")
    return (& curl.exe @curlArgs)
}

$init = Invoke-Mcp @{
    jsonrpc = "2.0"; id = 1; method = "initialize"
    params = @{ protocolVersion = "2025-06-18"; capabilities = @{}; clientInfo = @{ name = "smoke"; version = "1" } }
} $null
$init | Write-Host
$session = (Get-Content (Join-Path $temp "headers.txt") | Select-String -Pattern '^mcp-session-id: (.+)$' -CaseSensitive:$false).Matches.Groups[1].Value.Trim()

$tools = Invoke-Mcp @{ jsonrpc = "2.0"; id = 2; method = "tools/list"; params = @{} } $session
($tools | ConvertFrom-Json).result.tools | Select-Object name | Format-Table

$result = Invoke-Mcp @{
    jsonrpc = "2.0"; id = 3; method = "tools/call"
    params = @{
        name = "convert_document"
        arguments = @{ filename = "hello.txt"; source_format = "txt"; target_format = "pdf"; text = "Hello from MCP" }
    }
} $session
$result | Write-Host
