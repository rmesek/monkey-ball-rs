# Build the browser bundle into web/dist/ (git-ignored). Usage:  scripts\build-web.ps1 [-Dev] [-Single]
#   -Dev     unoptimised build (quick to compile, slow to run)
#   -Single  also write web/dist/monkey-ball.html: the whole game page in one file that opens from disk
#   needs: rustup target add wasm32-unknown-unknown
#          cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock> --locked
# web/dist/ is a static site: host it anywhere, or try it with the developer server:
#   cargo run -p xtask -- serve-web
param([switch]$Dev, [switch]$Single)

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
$targetDir = [System.IO.Path]::GetFullPath($targetDir)
$buildProfile = if ($Dev) { 'debug' } else { 'web' }
$cargoArgs = @('build', '-p', 'mb-web', '--target', 'wasm32-unknown-unknown', '--target-dir', $targetDir)
# The web profile (workspace Cargo.toml) is release plus fat LTO, one codegen unit and panic=abort.
if (-not $Dev) { $cargoArgs += @('--profile', 'web') }

& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { Fail 'cargo build failed' }

# Glue as a classic script (no ES modules): the same file then works in the page, in the import worker and
# inlined in the single-file build.
$wasm = Join-Path $targetDir "wasm32-unknown-unknown/$buildProfile/mb_web.wasm"
$pkg = Join-Path $targetDir 'web-pkg'
if (Test-Path $pkg) { Remove-Item -Recurse -Force $pkg }
& wasm-bindgen --target no-modules --no-typescript --out-dir $pkg $wasm
if ($LASTEXITCODE -ne 0) { Fail 'wasm-bindgen failed' }

$bundleArgs = @('run', '--quiet', '-p', 'xtask', '--target-dir', $targetDir, '--', 'bundle-web', '--pkg', $pkg)
if ($Single) {
    # The single file carries the module gzipped (the browser unpacks it with DecompressionStream).
    $gz = Join-Path $pkg 'mb_web_bg.wasm.gz'
    $in = [System.IO.File]::OpenRead((Join-Path $pkg 'mb_web_bg.wasm'))
    $out = [System.IO.File]::Create($gz)
    $zip = New-Object System.IO.Compression.GZipStream($out, [System.IO.Compression.CompressionLevel]::Optimal)
    $in.CopyTo($zip)
    $zip.Dispose(); $out.Dispose(); $in.Dispose()
    $bundleArgs += @('--single', '--wasm-gz', $gz)
}
& cargo @bundleArgs
if ($LASTEXITCODE -ne 0) { Fail 'bundling failed' }

$size = (Get-Item (Join-Path $root 'web/dist/pkg/mb_web_bg.wasm')).Length
Write-Host ("built web/dist ({0:N1} MiB wasm). Try it with: cargo run -p xtask -- serve-web" -f ($size / 1MB))
