# Download one hour of GH Archive (gzipped NDJSON, ~100-200k GitHub events)
# into fixtures/ for testing oxtail. Usage:
#   .\scripts\fetch-fixture.ps1              # default hour
#   .\scripts\fetch-fixture.ps1 2024-06-01-9 # any YYYY-MM-DD-H hour
param([string]$Hour = "2024-01-01-15")

$dir = Join-Path $PSScriptRoot "..\fixtures"
New-Item -ItemType Directory -Force $dir | Out-Null
$out = Join-Path $dir "$Hour.json.gz"

if (Test-Path $out) {
    Write-Host "Already downloaded: $out"
    exit 0
}

curl.exe -fL -o $out "https://data.gharchive.org/$Hour.json.gz"
Write-Host "Saved $out"
Write-Host "Try: cargo run --release -- fixtures/$Hour.json.gz --rate 50"
