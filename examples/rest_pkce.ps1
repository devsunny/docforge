<#
.SYNOPSIS
  Complete OAuth 2.1 authorization-code + PKCE flow against a local DocForge,
  then convert a text file through the REST API.

.DESCRIPTION
  This is a development walkthrough. It performs the demo login POST directly,
  which is convenient for CI/smoke tests but is not a substitute for a browser.
  For a real client, send the user to /oauth/authorize and handle the redirect.

.EXAMPLE
  ./rest_pkce.ps1 -BaseUrl http://127.0.0.1:8080 -Target pdf
#>
param(
    [string]$BaseUrl = "http://127.0.0.1:8080",
    [string]$Username = "demo",
    [string]$Password = "demo",
    [string]$Target = "pdf",
    [string]$InputFile = ""
)

$ErrorActionPreference = "Stop"
$BaseUrl = $BaseUrl.TrimEnd('/')
$redirect = "http://127.0.0.1:9876/callback"

function New-RandomUrlSafe([int]$Bytes) {
    if ([System.Security.Cryptography.RandomNumberGenerator].GetMethods() -match 'GetBytes\(System.Int32\)') {
        $raw = [System.Security.Cryptography.RandomNumberGenerator]::GetBytes($Bytes)
    } else {
        $rng = New-Object System.Security.Cryptography.RNGCryptoServiceProvider
        $raw = New-Object byte[] $Bytes
        $rng.GetBytes($raw)
    }
    [Convert]::ToBase64String($raw).Replace('+', '-').Replace('/', '_').TrimEnd('=')
}

Write-Host "1/5 Registering a public PKCE client..." -ForegroundColor Cyan
$registration = @{
    client_name                = "DocForge PowerShell example"
    redirect_uris              = @($redirect)
    grant_types                = @("authorization_code", "refresh_token")
    response_types             = @("code")
    token_endpoint_auth_method = "none"
    scope                      = "convert mcp offline_access"
} | ConvertTo-Json -Depth 5
$client = Invoke-RestMethod -Method Post -Uri "$BaseUrl/oauth/register" `
    -ContentType "application/json" -Body $registration
$clientId = $client.client_id

$verifier = New-RandomUrlSafe 48
$sha = [System.Security.Cryptography.SHA256]::Create()
$challenge = [Convert]::ToBase64String(
    $sha.ComputeHash([Text.Encoding]::ASCII.GetBytes($verifier))
).Replace('+', '-').Replace('/', '_').TrimEnd('=')

Write-Host "2/5 Authorizing (demo credentials)..." -ForegroundColor Cyan
$form = @{
    response_type         = "code"
    client_id             = $clientId
    redirect_uri          = $redirect
    scope                 = "convert offline_access"
    state                 = "example"
    code_challenge        = $challenge
    code_challenge_method = "S256"
    resource              = "$BaseUrl/api"
    username              = $Username
    password              = $Password
}
$curlArgs = @("-s", "-i", "-X", "POST", "$BaseUrl/oauth/authorize", "-H", "Content-Type: application/x-www-form-urlencoded")
foreach ($key in $form.Keys) { $curlArgs += @("--data-urlencode", "$key=$($form[$key])") }
$response = & curl.exe @curlArgs
$location = ($response | Select-String -Pattern '^Location: (.+)$').Matches.Groups[1].Value.Trim()
$code = [regex]::Match($location, '[?&]code=([^&]+)').Groups[1].Value
if (-not $code) { throw "Authorization failed: $location" }

Write-Host "3/5 Exchanging the code for tokens..." -ForegroundColor Cyan
$token = Invoke-RestMethod -Method Post -Uri "$BaseUrl/oauth/token" `
    -ContentType "application/x-www-form-urlencoded" -Body @{
        grant_type    = "authorization_code"
        code          = $code
        redirect_uri  = $redirect
        client_id     = $clientId
        code_verifier = $verifier
        resource      = "$BaseUrl/api"
    }
Write-Host "    access token: $($token.access_token.Length) chars, scope=$($token.scope)"

Write-Host "4/5 Calling a protected endpoint..." -ForegroundColor Cyan
$headers = @{ Authorization = "Bearer $($token.access_token)" }
$formats = Invoke-RestMethod -Uri "$BaseUrl/api/v1/formats" -Headers $headers
Write-Host "    $($formats.count) formats available"

Write-Host "5/5 Converting..." -ForegroundColor Cyan
if (-not $InputFile) {
    $InputFile = Join-Path ([System.IO.Path]::GetTempPath()) "docforge-example.txt"
    @"
DocForge example

This file was converted through the OAuth 2.1 REST API.

- PKCE S256
- Ed25519 JWT
- Multi-hop conversion graph
"@ | Set-Content -Path $InputFile -Encoding UTF8
}
$output = Join-Path (Get-Location) ("docforge-example." + $Target)
& curl.exe -s -X POST "$BaseUrl/api/v1/convert" `
    -H "Authorization: Bearer $($token.access_token)" `
    -F "file=@$InputFile" `
    -F "target_format=$Target" `
    -o $output
Write-Host "    wrote $output ($((Get-Item $output).Length) bytes)" -ForegroundColor Green
