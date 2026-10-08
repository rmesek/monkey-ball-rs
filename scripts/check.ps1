# The quality gates (AGENTS.md), run locally: free, on your machine. The PowerShell twin of check.sh.
# Tests that need the game read your own disc image (MB_DISC, or "Super Monkey Ball*.rvz|iso|gcm" next to the
# repository) and skip without it. The browser check is separate: scripts\build-web.ps1, then
# deno run -A scripts/web-e2e.ts.
# cargo reports progress on stderr; only its exit code decides (with 'Stop', Windows PowerShell would turn the
# first stderr line into a failure whenever the output is redirected).
$ErrorActionPreference = 'Continue'
Set-Location (Join-Path $PSScriptRoot '..')

function Step([string[]]$command) {
    Write-Host "> $($command -join ' ')"
    $exe, $rest = $command
    & $exe @rest
    if ($LASTEXITCODE -ne 0) { Write-Error "failed: $($command -join ' ')"; exit 1 }
}

if (Test-Path .git) {
    $tracked = git ls-files | Where-Object { $_ -match '\.(iso|gcm|rvz|wia|ciso|gcz|gma|tpl|lz|adp|pack|ogg|wav)$' }
    if ($tracked) { $tracked; Write-Error 'Game data must not be tracked'; exit 1 }
}
Step @('cargo', 'fmt', '--all', '--', '--check')
Step @('cargo', 'clippy', '--workspace', '--all-targets', '--', '-D', 'warnings')
# The product configurations: the desktop program without the dev feature, and the browser build.
Step @('cargo', 'clippy', '-p', 'mb-app', '--', '-D', 'warnings')
Step @('cargo', 'clippy', '-p', 'mb-web', '--target', 'wasm32-unknown-unknown', '--target-dir', 'target/wasm', '--', '-D', 'warnings')
Step @('cargo', 'test', '--workspace', '--release')
