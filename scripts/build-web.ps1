# Build the browser version into web/pkg/ (git-ignored). Usage:  scripts\build-web.ps1 [-Dev]
#   needs: rustup target add wasm32-unknown-unknown
#          cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock> --locked
# Then serve it:  cargo run -p xtask -- serve-web
param([switch]$Dev)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Fail($msg) { Write-Host "build-web: $msg" -ForegroundColor Red; exit 1 }

# The wasm-bindgen CLI must be exactly the version of the wasm-bindgen crate the build links.
$lock = Get-Content (Join-Path $root 'Cargo.lock') -Raw
if ($lock -notmatch '(?m)^name = "wasm-bindgen"\r?\nversion = "([^"]+)"') { Fail 'wasm-bindgen is not in Cargo.lock' }
$want = $Matches[1]
$cli = Get-Command wasm-bindgen -ErrorAction SilentlyContinue
if (-not $cli) { Fail "wasm-bindgen CLI not found. Install it with: cargo install wasm-bindgen-cli --version $want --locked" }
$have = ((& wasm-bindgen --version) -split ' ')[1]
if ($have -ne $want) {
    Fail "wasm-bindgen CLI is $have but the crate is $want. Run: cargo install wasm-bindgen-cli --version $want --locked"
}

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$buildProfile = if ($Dev) { 'debug' } else { 'release' }
$cargoArgs = @('build', '-p', 'mb-web', '--target', 'wasm32-unknown-unknown', '--target-dir', $targetDir)
if (-not $Dev) { $cargoArgs += '--release' }

& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { Fail 'cargo build failed' }

$wasm = Join-Path $targetDir "wasm32-unknown-unknown/$buildProfile/mb_web.wasm"
$out = Join-Path $root 'web/pkg'
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
& wasm-bindgen --target web --no-typescript --out-dir $out $wasm
if ($LASTEXITCODE -ne 0) { Fail 'wasm-bindgen failed' }

$size = (Get-Item (Join-Path $out 'mb_web_bg.wasm')).Length
Write-Host ("built web/pkg ({0:N1} MiB wasm). Serve it with: cargo run -p xtask -- serve-web" -f ($size / 1MB))
