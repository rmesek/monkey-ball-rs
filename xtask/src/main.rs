//! The project's build tool: one entry point on every platform, no shell scripts.
//!
//! ```text
//! cargo xtask build-web [--dev]    build the static web site into target/site/
//! cargo xtask ci                    run the quality gates (AGENTS.md)
//! ```
//! `cargo xtask` is the alias of the cargo-xtask convention (`.cargo/config.toml`) for
//! `cargo run --release -p xtask --`. The tool builds; it serves nothing.

use std::path::{Path, PathBuf};
use std::process::Command;

mod bundle;

const USAGE: &str = "usage:
  cargo xtask build-web [--dev]
      build the browser target and assemble the static site in target/site/, to be hosted by any static
      file server: python -m http.server -d target/site 8080
        --dev     unoptimised build (quick to compile, slow to run; keeps function names for stack traces)
  cargo xtask ci
      the quality gates: no game data tracked, cargo fmt, clippy (workspace, desktop program as shipped,
      browser build, the game library for Android) and cargo test --workspace --release";

/// Files that are game data whatever their content: they must never be tracked.
const GAME_DATA_EXTENSIONS: [&str; 12] = [
    "iso", "gcm", "rvz", "wia", "ciso", "gcz", "gma", "tpl", "lz", "adp", "pack", "wav",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace root")
        .to_path_buf()
}

/// The most the optimised module may weigh. Every phone downloads and compiles these bytes, so they
/// are not to grow by accident: the web build fails above this.
///
/// To raise it on purpose: change the allowance here in a commit of its own that says what the bytes buy,
/// and put the new figure into the budget table of `docs/PLAN.md`. First look for
/// what brought the bytes in (a new dependency, a generic instantiated many times, formatting code).
const WASM_BUDGET_BYTES: u64 = 4_127_298;

/// The line `build-web` prints about the module's size, or the error when it is over `budget`.
fn wasm_size_report(size: u64, budget: u64) -> Result<String, String> {
    if size > budget {
        return Err(format!(
            "pkg/mb_web_bg.wasm is {size} bytes, {} over its budget of {budget}. Find what grew, or raise \
             WASM_BUDGET_BYTES in xtask/src/main.rs on purpose (its comment says how). The built site was \
             left as it was.",
            size - budget
        ));
    }
    Ok(format!(
        "wasm module: {size} bytes, budget {budget}, {} bytes of headroom",
        budget - size
    ))
}

/// The cargo that is running this tool, so nested builds use the same toolchain.
fn cargo() -> Command {
    let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    c.current_dir(root());
    c
}

/// Run a command to completion, echoing it first; an error names the command.
fn run(mut command: Command) -> Result<(), String> {
    // As one would type it: the program's name, not its full path, and no quoting noise.
    let program = Path::new(command.get_program())
        .file_stem()
        .unwrap_or(command.get_program())
        .to_string_lossy()
        .into_owned();
    let shown = command
        .get_args()
        .fold(program, |line, arg| line + " " + &arg.to_string_lossy());
    println!("> {shown}");
    let status = command
        .status()
        .map_err(|e| format!("could not start {shown}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("failed ({status}): {shown}"))
    }
}

/// Build the site and return its directory: `site` in cargo's target directory. It is build output
/// like the rest of what is there (`cargo clean` removes it); the sources of the page are `web/`.
fn build_web(dev: bool) -> Result<PathBuf, String> {
    let root = root();
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { root.join(p) })
        .unwrap_or_else(|| root.join("target"));

    // The `web` profile (workspace Cargo.toml) is release plus fat LTO, one codegen unit and panic=abort.
    let mut build = cargo();
    build.args([
        "build",
        "-p",
        "mb-web",
        "--target",
        "wasm32-unknown-unknown",
    ]);
    build.arg("--target-dir").arg(&target_dir);
    if !dev {
        build.args(["--profile", "web"]);
    }
    run(build)?;

    // Glue as a classic script (no ES modules): the same file then works in the page and in the import
    // worker.
    let pkg = target_dir.join("web-pkg");
    if pkg.exists() {
        std::fs::remove_dir_all(&pkg).map_err(|e| format!("{}: {e}", pkg.display()))?;
    }
    // wasm-bindgen's own post-processing, linked into this tool at the version of the crate the game
    // links (both are pinned), so there is no command to install and none to get out of step.
    let mut bindgen = wasm_bindgen_cli_support::Bindgen::new();
    bindgen
        .input_path(
            target_dir
                .join("wasm32-unknown-unknown")
                .join(if dev { "debug" } else { "web" })
                .join("mb_web.wasm"),
        )
        .no_modules(true)
        .map_err(|e| format!("wasm-bindgen: {e:#}"))?
        .typescript(false)
        // Without the function-name section (about a fifth of the module; a panic still reports its
        // message and source line).
        .remove_name_section(!dev)
        .remove_producers_section(!dev);
    println!("> wasm-bindgen (no-modules) -> {}", pkg.display());
    bindgen
        .generate(&pkg)
        .map_err(|e| format!("wasm-bindgen: {e:#}"))?;

    // The size budget holds for the build that ships; an unoptimised module is several times the size.
    let wasm = pkg.join("mb_web_bg.wasm");
    let size = std::fs::metadata(&wasm)
        .map_err(|e| format!("{}: {e}", wasm.display()))?
        .len();
    if dev {
        println!("wasm module: {size} bytes (unoptimised: the size budget is not applied)");
    } else {
        println!("{}", wasm_size_report(size, WASM_BUDGET_BYTES)?);
    }

    let out = target_dir.join("site");
    bundle::run(bundle::Options {
        web: root.join("web"),
        pkg,
        out: out.clone(),
    })?;
    println!("built {} ({} KiB wasm)", out.display(), size / 1024);
    Ok(out)
}

/// The one option of the web build.
fn web_options(args: impl Iterator<Item = String>) -> Result<bool, String> {
    let mut dev = false;
    for arg in args {
        match arg.as_str() {
            "--dev" => dev = true,
            other => return Err(format!("unknown option {other}\n{USAGE}")),
        }
    }
    Ok(dev)
}

/// Tracked files that look like game data (by extension).
fn tracked_game_data(listing: &str) -> Vec<&str> {
    listing
        .lines()
        .filter(|path| {
            Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| GAME_DATA_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect()
}

fn ci() -> Result<(), String> {
    // Not every checkout is a git repository (a source archive is not); then there is nothing tracked.
    if let Ok(out) = Command::new("git")
        .arg("ls-files")
        .current_dir(root())
        .output()
        && out.status.success()
    {
        let listing = String::from_utf8_lossy(&out.stdout);
        let bad = tracked_game_data(&listing);
        if !bad.is_empty() {
            return Err(format!("game data must not be tracked: {}", bad.join(", ")));
        }
    }
    let gates: [&[&str]; 6] = [
        &["fmt", "--all", "--", "--check"],
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        // The product configurations: the desktop program without the dev feature, and the browser build.
        &["clippy", "-p", "mb-desktop", "--", "-D", "warnings"],
        &[
            "clippy",
            "-p",
            "mb-web",
            "--target",
            "wasm32-unknown-unknown",
            "--target-dir",
            "target/wasm",
            "--",
            "-D",
            "warnings",
        ],
        // The game library and everything under it for a native Android target: no app is built from
        // it yet, but nothing shared may stop compiling there (docs/ARCHITECTURE.md). Needs only the
        // target's standard library (rust-toolchain.toml), no NDK.
        &[
            "clippy",
            "-p",
            "mb-app",
            "--target",
            "aarch64-linux-android",
            "--target-dir",
            "target/android",
            "--",
            "-D",
            "warnings",
            // False there: Android has no native thread-locals, and the lint does not see the `const`
            // through the expansion `thread_local!` has on such targets.
            "-A",
            "clippy::missing_const_for_thread_local",
        ],
        &["test", "--workspace", "--release"],
    ];
    for gate in gates {
        let mut c = cargo();
        c.args(gate);
        run(c)?;
    }
    println!("all gates passed");
    Ok(())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let result = match args.next().as_deref() {
        Some("build-web") => web_options(args).and_then(|dev| {
            build_web(dev)?;
            println!(
                "host that folder with any static file server: python -m http.server -d target/site 8080"
            );
            Ok(())
        }),
        Some("ci") if args.next().is_none() => ci(),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("xtask: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The versions of `name` in `Cargo.lock`.
    fn locked_versions<'a>(lock: &'a str, name: &str) -> Vec<&'a str> {
        let wanted = format!("name = \"{name}\"");
        let mut lines = lock.lines().map(str::trim);
        let mut found = Vec::new();
        while let Some(line) = lines.next() {
            if line == wanted
                && let Some(v) = lines.next().and_then(|l| l.strip_prefix("version = \""))
            {
                found.push(v.trim_end_matches('"'));
            }
        }
        found
    }

    #[test]
    fn the_bindgen_in_this_tool_is_the_version_the_game_links() {
        let lock = "[[package]]\nname = \"wasm-bindgen-futures\"\nversion = \"0.4.1\"\n\n\
                    [[package]]\r\nname = \"wasm-bindgen\"\r\nversion = \"0.2.129\"\r\nsource = \"x\"\n";
        assert_eq!(locked_versions(lock, "wasm-bindgen"), ["0.2.129"]);
        assert!(locked_versions(lock, "other").is_empty());
        // The glue this tool writes is tied to the crate's version: one of each, and the same.
        let real = std::fs::read_to_string(root().join("Cargo.lock")).unwrap();
        let game = locked_versions(&real, "wasm-bindgen");
        assert_eq!(game.len(), 1);
        assert_eq!(locked_versions(&real, "wasm-bindgen-cli-support"), game);
    }

    #[test]
    fn the_wasm_module_is_held_to_its_size_budget() {
        let line = wasm_size_report(4_006_367, WASM_BUDGET_BYTES).unwrap();
        assert!(line.contains("4006367 bytes"), "{line}");
        assert!(line.contains("120931 bytes of headroom"), "{line}");
        // Exactly the budget passes with nothing left; one byte more fails and says by how much.
        let line = wasm_size_report(WASM_BUDGET_BYTES, WASM_BUDGET_BYTES).unwrap();
        assert!(line.contains(" 0 bytes of headroom"), "{line}");
        let e = wasm_size_report(WASM_BUDGET_BYTES + 1, WASM_BUDGET_BYTES).unwrap_err();
        assert!(
            e.contains("4127299 bytes, 1 over its budget of 4127298"),
            "{e}"
        );
        assert!(e.contains("WASM_BUDGET_BYTES"), "{e}");
    }

    #[test]
    fn game_data_is_recognised_by_extension_only() {
        let listing = "README.md\ncrates/a/src/lib.rs\nstuff/STAGE001.lz\nx/Game.RVZ\nweb/icon.svg\nnotes.pack.md\n";
        assert_eq!(
            tracked_game_data(listing),
            ["stuff/STAGE001.lz", "x/Game.RVZ"]
        );
    }
}
