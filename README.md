# monkey-ball-rs

A cross-platform (desktop, Android, browser/WASM), fully offline Super Monkey Ball engine written in Rust.
It is a parser plus game engine that mirrors the working of [WebMonkeyBall](../WebMonkeyBall) and the web build
it powers. SMB1 comes first; SMB2 and the other sources follow.

## Offline only
No network or multiplayer features. UI is designed for touch, mobile and browser as well as desktop.

## Status
Phase 0 scaffold. See [docs/PLAN.md](docs/PLAN.md).

## Game content is not included
This repository contains **code only**. It never contains, downloads, or links to original game data.
The engine reads content from a git-ignored **content root**, laid out like the web build:

```
content/
  smb1_content/test/{init,bg,stNNN}/...   stagedefs, models, textures
  audio/{sfx,smb1-music-compressed}/...   wav / ogg
  assets/hud/smb1/{bmp_com,bmp_nml}/...   png
```

You supply the content root yourself. See [docs/GETTING_ASSETS.md](docs/GETTING_ASSETS.md) and [docs/ASSET_LOADING.md](docs/ASSET_LOADING.md).

## Workspace
| Path | Purpose |
|---|---|
| `crates/mb-assets` | content layout, pack loading, format parsers (no GPU deps) |
| `crates/mb-engine` | deterministic simulation core |
| `xtask` | dev commands, e.g. `check-content` |

Graphics, audio, windowing and serialization crates are deliberately **not chosen yet**.

## Build and check
```bash
cargo test --workspace
cargo run -p xtask -- check-content content 1 bg_jun
```

## Credits (from the reference project)
Original game: Amusement Vision. SMB1 decompilation: camthesaxman. Web port: TwilightPB. Renderer: ComplexPlane.
Community tools and resources: SMB Custom Level Community.
