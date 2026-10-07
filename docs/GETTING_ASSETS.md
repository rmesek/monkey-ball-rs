# Getting the game content

## Quickest path: use your disc image directly
The app reads files straight out of a disc image of your own copy (`.rvz`, `.iso`/`.gcm`, `.ciso`, `.wia`, ...)
through `mb_assets::disc::DiscPack`. No extraction, no zip, nothing written to disk.

```bash
cargo run --release -p xtask -- disc-info  "Super Monkey Ball (USA).rvz"
cargo run --release -p xtask -- check-disc "Super Monkey Ball (USA).rvz" 1 bg_jun
```
Verified on a USA (`GMBE8P`) RVZ: 897 files, 896 under `test/`; all stage-1 and stage-2 files present and
`STAGE001.lz` decompresses (45,596 -> 127,166 bytes). The older sections below remain for folder/zip content
and for the audio/HUD files, which are not read from the disc yet.

This project ships **no game data** and downloads none. You must supply content from a copy of the game you
legally own. Do not obtain it from sites that host the original files; those hosts are unlicensed and using
them is redistribution of copyrighted work.

## 1. What you need
A content folder in this layout (SMB1; see [ASSET_LOADING.md](ASSET_LOADING.md)):

```
<content>/
  smb1_content/test/{init,bg,stNNN}/...     models, textures, stagedefs (.lz/.gma/.tpl)
  audio/sfx/*.wav
  audio/smb1-music-compressed/*.ogg
  assets/hud/smb1/{bmp_com,bmp_nml}/*.png
```

Status of each part (be aware of the gaps):
- **`smb1_content/test/...`**: the game's own file tree, copied unchanged. This is what the loaders and parsers read.
- **Audio and HUD**: the reference web build expects pre-converted `.wav`/`.ogg`/`.png` files, and nothing in the
  reference repository creates them. How the original sounds and sprites map to these files is not documented in
  the reference. Phase 3 will define importers or accept pre-converted files; until then the engine runs without
  them (silent, no HUD sprites).
- Exact disc folder names and the region/version the reference was built against are not stated in the
  reference repository. Run the validator (below) to see precisely which files are missing.

## 2. Where it comes from
Only from your own copy of Super Monkey Ball (GameCube release):
1. **Dump your own disc.** Use a Wii or GameCube you own with a homebrew disc dumper to produce an image of
   *your* disc. (Discs must be dumped by you; do not download disc images.)
2. **Extract the file tree from your image** with any disc-image tool you trust, for example the "Extract Entire
   Disc" option in the Dolphin emulator, or `nodtool extract` from the `nod` project. These tools are *outside*
   this repository and are not part of the build. Copy the extracted files into the layout above
   (the reference places the extracted files under `smb1_content/test`).
3. **Convert audio/HUD** with tools of your choice until Phase 3 provides importers.

### Script (Windows)
Disc IDs for the GameCube Super Monkey Ball 1 (verify your dump against the Redump database yourself):
`GMBE8P` (USA), `GMBP8P` (Europe), `GMBJ8P` (Japan). Super Monkey Ball 2 (`GM2…`) is a different disc.

```powershell
cargo install nodtool                                  # external extractor, one time
.\scripts\extract-content.ps1 -Iso C:\path\to\your_dump.iso
# or, if you extracted with Dolphin yourself:
.\scripts\extract-content.ps1 -FilesDir C:\path\to\extracted\files
```
It checks the disc ID (raw `.iso`/`.gcm`), extracts, copies the disc's `test` folder to
`content/smb1_content/test` and runs the validator. If the extracted tree has no `test` folder it stops and tells you.

## 3. Check what you have
```bash
cargo run -p xtask -- check-content <content> 1 bg_jun
```
It lists every file still missing for stage 1. Stage backgrounds differ per stage; pass the right `bg_*` name.

## 4. Mobile (Android / iOS)
1. Put the content folder (or one zip of it) on the phone: USB copy, cloud drive, or a file manager.
2. First launch shows "Select your content". Pick the folder with the system picker (no storage permission needed).
3. The app validates it (`mb_assets::content::validate_files`), then copies it into app-private storage
   (`DirStore::import_from_dir`) with a progress bar and an "import complete" marker.
4. After that the game runs offline from the private copy; you can delete the original.

## 5. Browser
Pick the folder (or zip) in the page; files are stored in the browser's private storage (OPFS) and reused offline.
