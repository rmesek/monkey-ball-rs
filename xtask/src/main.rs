//! The project's build tool: one entry point on every platform, standard library only.
//!
//! ```text
//! cargo xtask build-web [--dev]               build the static web site into web/dist/
//! cargo xtask check                           run the quality gates (AGENTS.md)
//! ```
//! `cargo xtask` is an alias for `cargo run --quiet -p xtask --` (`.cargo/config.toml`).

use std::path::{Path, PathBuf};
use std::process::Command;

mod bundle;

const USAGE: &str = "usage:
  cargo xtask build-web [--dev]
      build the browser target and assemble the static site in web/dist/
        --dev     unoptimised build (quick to compile, slow to run; keeps function names for stack traces)
      needs the wasm-bindgen command at the version of the wasm-bindgen crate in Cargo.lock
  cargo xtask check
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

/// Size of `pkg/mb_web_bg.wasm` as version 0.1.0 shipped it (the optimised build, name section removed).
const WASM_BYTES_0_1_0: u64 = 3_877_298;
/// The most the optimised module may weigh: what `docs/PARITY_PLAN.md` ("Limits every phase works
/// inside", Size) allows over 0.1.0. Every phone downloads and compiles these bytes, so they are not to
/// grow by accident: `cargo xtask build-web` fails above this.
///
/// To raise it on purpose: change the allowance here in a commit of its own that says what the bytes buy,
/// and put the new figure into `docs/PARITY_PLAN.md` and the size table of `docs/WEB.md`. First look for
/// what brought the bytes in (a new dependency, a generic instantiated many times, formatting code).
const WASM_BUDGET_BYTES: u64 = WASM_BYTES_0_1_0 + 250_000;

/// The line `build-web` prints about the module's size, or the error when it is over `budget`.
fn wasm_size_report(size: u64, budget: u64) -> Result<String, String> {
    let growth = size as i64 - WASM_BYTES_0_1_0 as i64;
    if size > budget {
        return Err(format!(
            "pkg/mb_web_bg.wasm is {size} bytes, {} over its budget of {budget} ({growth:+} against the              {WASM_BYTES_0_1_0} of 0.1.0). Find what grew, or raise WASM_BUDGET_BYTES in xtask/src/main.rs              on purpose (its comment says how). web/dist was left as it was.",
            size - budget
        ));
    }
    Ok(format!(
        "wasm module: {size} bytes ({growth:+} against 0.1.0), budget {budget}, {} bytes of headroom",
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

/// The version of the `wasm-bindgen` crate the build links, from `Cargo.lock`.
fn locked_wasm_bindgen(lock: &str) -> Option<&str> {
    let mut lines = lock.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if line == "name = \"wasm-bindgen\"" {
            return lines
                .next()?
                .strip_prefix("version = \"")?
                .strip_suffix('"');
        }
    }
    None
}

/// The wasm-bindgen command must be exactly the version of the crate: the glue it writes is tied to it.
fn check_wasm_bindgen() -> Result<(), String> {
    let lock = std::fs::read_to_string(root().join("Cargo.lock"))
        .map_err(|e| format!("Cargo.lock: {e}"))?;
    let want = locked_wasm_bindgen(&lock).ok_or("wasm-bindgen is not in Cargo.lock")?;
    let install = format!("cargo install wasm-bindgen-cli --version {want} --locked");
    let out = Command::new("wasm-bindgen")
        .arg("--version")
        .output()
        .map_err(|_| {
            format!("the wasm-bindgen command was not found. Install it with: {install}")
        })?;
    let text = String::from_utf8_lossy(&out.stdout);
    let have = text.split_whitespace().nth(1).unwrap_or("");
    if have == want {
        Ok(())
    } else {
        Err(format!(
            "the wasm-bindgen command is version {have:?} but the crate is {want}. Run: {install}"
        ))
    }
}

fn build_web(args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut dev = false;
    for arg in args {
        match arg.as_str() {
            "--dev" => dev = true,
            other => return Err(format!("unknown option {other}\n{USAGE}")),
        }
    }
    check_wasm_bindgen()?;
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
    let mut bindgen = Command::new("wasm-bindgen");
    bindgen.args(["--target", "no-modules", "--no-typescript"]);
    if !dev {
        // Drops the function-name section (about a fifth of the module; a panic still reports its message
        // and source line).
        bindgen.args(["--remove-name-section", "--remove-producers-section"]);
    }
    bindgen.arg("--out-dir").arg(&pkg).arg(
        target_dir
            .join("wasm32-unknown-unknown")
            .join(if dev { "debug" } else { "web" })
            .join("mb_web.wasm"),
    );
    run(bindgen)?;

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

    let out = root.join("web").join("dist");
    bundle::run(bundle::Options {
        web: root.join("web"),
        pkg,
        out: out.clone(),
    })?;
    println!(
        "built web/dist ({} KiB wasm). Serve it with any static file server, for example:\n  \
         cd web/dist\n  python -m http.server 8080",
        size / 1024
    );
    Ok(())
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

fn check() -> Result<(), String> {
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
        Some("build-web") => build_web(args),
        Some("check") if args.next().is_none() => check(),
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

    #[test]
    fn reads_the_wasm_bindgen_version_from_the_lock_file() {
        let lock = "[[package]]\nname = \"wasm-bindgen-futures\"\nversion = \"0.4.1\"\n\n\
                    [[package]]\r\nname = \"wasm-bindgen\"\r\nversion = \"0.2.129\"\r\nsource = \"x\"\n";
        assert_eq!(locked_wasm_bindgen(lock), Some("0.2.129"));
        assert_eq!(
            locked_wasm_bindgen("name = \"other\"\nversion = \"1\"\n"),
            None
        );
        // The real lock file has it.
        let real = std::fs::read_to_string(root().join("Cargo.lock")).unwrap();
        assert!(locked_wasm_bindgen(&real).is_some());
    }

    #[test]
    fn the_wasm_module_is_held_to_its_size_budget() {
        // The plan's figure: a quarter of a megabyte over 0.1.0.
        assert_eq!(WASM_BUDGET_BYTES, 4_127_298);
        let line = wasm_size_report(4_006_367, WASM_BUDGET_BYTES).unwrap();
        assert!(line.contains("4006367 bytes"), "{line}");
        assert!(line.contains("+129069 against 0.1.0"), "{line}");
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
        // A module smaller than 0.1.0 is reported as such.
        let line = wasm_size_report(3_800_000, WASM_BUDGET_BYTES).unwrap();
        assert!(line.contains("-77298 against 0.1.0"), "{line}");
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
