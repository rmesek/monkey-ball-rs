<#
Extract SMB1 game files from YOUR OWN disc image into the content layout.

Usage:  .\scripts\extract-content.ps1 -Iso path\to\your_dump.iso [-Out content]

Needs an external extractor (not part of this repo): `nodtool` (cargo install nodtool).
Dolphin's "Extract Entire Disc" also works; extract by hand, then pass -FilesDir instead of -Iso.
#>
param(
    [string]$Iso,
    [string]$FilesDir,
    [string]$Out = "content"
)
$ErrorActionPreference = "Stop"
$known = @{ "GMBE8P" = "USA (NTSC-U)"; "GMBP8P" = "Europe (PAL)"; "GMBJ8P" = "Japan (NTSC-J)" }

if (-not $FilesDir) {
    if (-not $Iso) { throw "Pass -Iso <image> or -FilesDir <already extracted files folder>" }
    if (-not (Test-Path $Iso)) { throw "Not found: $Iso" }

    if ($Iso -match '\.(iso|gcm)$') {
        $fs = [System.IO.File]::OpenRead($Iso)
        $buf = New-Object byte[] 6
        [void]$fs.Read($buf, 0, 6); $fs.Close()
        $id = [System.Text.Encoding]::ASCII.GetString($buf)
        if ($known.ContainsKey($id)) { Write-Host "Disc ID $id = Super Monkey Ball 1, $($known[$id])" }
        else { throw "Disc ID '$id' is not a known Super Monkey Ball 1 ID ($($known.Keys -join ', '))" }
    } else {
        Write-Host "Compressed image: disc ID check skipped (nodtool info will show it)."
    }

    if (-not (Get-Command nodtool -ErrorAction SilentlyContinue)) {
        throw "nodtool not found. Install with: cargo install nodtool"
    }
    $tmp = Join-Path $Out "_extract"
    if (Test-Path $tmp) { Remove-Item $tmp -Recurse -Force }
    nodtool extract $Iso $tmp
    $FilesDir = if (Test-Path (Join-Path $tmp "files")) { Join-Path $tmp "files" } else { $tmp }
}

# The web build expects the disc's `test` folder at smb1_content/test.
$src = Join-Path $FilesDir "test"
if (-not (Test-Path $src)) {
    throw "No 'test' folder in $FilesDir. Inspect the extracted files and copy them to $Out\smb1_content\test by hand."
}
$dst = Join-Path $Out "smb1_content\test"
New-Item -ItemType Directory -Force $dst | Out-Null
Copy-Item (Join-Path $src "*") $dst -Recurse -Force
Write-Host "Copied to $dst"

cargo run -q -p xtask -- check-content $Out 1 bg_jun
