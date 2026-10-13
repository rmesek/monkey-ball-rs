//! The last step of `cargo run --release -p xtask -- build-web`: assemble the static web bundle in `target/site/` from the page
//! sources in `web/` and the wasm-bindgen output. Standard library only.
//!
//! The bundle is `index.html`, `guard.js`, `main.js`, `worker.js`, `sw.js` (with a version and the precache
//! list filled in), the manifest, the icons and `pkg/`. All paths relative, and nothing else: only the files
//! named here are copied, whatever else lies in `web/`.

use std::fs;
use std::path::{Path, PathBuf};

/// Files copied verbatim from `web/` into the bundle (and precached by the service worker).
const STATIC_FILES: [&str; 7] = [
    "guard.js",
    "main.js",
    "worker.js",
    "manifest.webmanifest",
    "icon.svg",
    "icon-192.png",
    "icon-512.png",
];
const PKG_FILES: [&str; 2] = ["pkg/mb_web.js", "pkg/mb_web_bg.wasm"];
/// Where `index.html` gets the build's version.
const VERSION_MARK: &str = "<!--build:version-->";

pub struct Options {
    /// The page sources (`web/`).
    pub web: PathBuf,
    /// Directory holding `mb_web.js` and `mb_web_bg.wasm` (wasm-bindgen's `--target no-modules` output).
    pub pkg: PathBuf,
    pub out: PathBuf,
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A text source with Unix line ends (a checkout on Windows may have converted them; the bundle and its
/// version are then the same wherever it is built).
fn read_text(path: &Path) -> Result<String, String> {
    let bytes = read(path)?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?;
    Ok(text.replace("\r\n", "\n"))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// FNV-1a, 64 bits: names the build. It only has to change when a file of the bundle does.
struct Fnv(u64);

impl Fnv {
    fn update(&mut self, data: &[u8]) {
        for b in data {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

pub fn run(opts: Options) -> Result<(), String> {
    let index = read_text(&opts.web.join("index.html"))?;
    if !index.contains(VERSION_MARK) {
        return Err(format!("index.html has no {VERSION_MARK}"));
    }
    let pkg_file = |name: &str| opts.pkg.join(name.trim_start_matches("pkg/"));
    for name in PKG_FILES {
        if !pkg_file(name).is_file() {
            return Err(format!(
                "{} is missing (wasm-bindgen should have written it)",
                pkg_file(name).display()
            ));
        }
    }
    if opts.out.exists() {
        fs::remove_dir_all(&opts.out).map_err(|e| format!("{}: {e}", opts.out.display()))?;
    }

    let mut hasher = Fnv(0xcbf2_9ce4_8422_2325);
    let mut put = |name: &str, bytes: &[u8]| -> Result<(), String> {
        hasher.update(name.as_bytes());
        hasher.update(&(bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
        write(&opts.out.join(name), bytes)
    };
    // The page itself is hashed before the version is stamped into it, and written afterwards.
    put("index.html", index.as_bytes())?;
    for name in STATIC_FILES {
        let path = opts.web.join(name);
        // Scripts and the manifest are text (normalised line ends); the PNGs are copied as they are.
        if name.ends_with(".png") {
            put(name, &read(&path)?)?;
        } else {
            put(name, read_text(&path)?.as_bytes())?;
        }
    }
    for name in PKG_FILES {
        put(name, &read(&pkg_file(name))?)?;
    }
    let version = format!("{:016x}", hasher.0);
    // Every reference to a bundle file carries the version, so a server that sends no cache headers
    // (python -m http.server) cannot pair an old script with a new engine.
    let stamp = format!("<meta name=\"mb-version\" content=\"{version}\">");
    let mut page = index.replace(VERSION_MARK, &stamp);
    for file in ["./guard.js", "./pkg/mb_web.js", "./main.js"] {
        let reference = format!("src=\"{file}\"");
        if !page.contains(&reference) {
            return Err(format!("index.html no longer loads {file}"));
        }
        page = page.replace(&reference, &format!("src=\"{file}?v={version}\""));
    }
    write(&opts.out.join("index.html"), page.as_bytes())?;
    let mut files = vec!["./".to_string(), "index.html".to_string()];
    files.extend(STATIC_FILES.iter().chain(&PKG_FILES).map(|f| f.to_string()));
    let list = files
        .iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let sw = read_text(&opts.web.join("sw.js"))?;
    if !(sw.contains("\"__MB_VERSION__\"") && sw.contains("__MB_FILES__")) {
        return Err("sw.js has lost its __MB_VERSION__ / __MB_FILES__ placeholders".into());
    }
    let sw = sw
        .replace("\"__MB_VERSION__\"", &format!("\"{version}\""))
        .replace("__MB_FILES__", &format!("[{list}]"));
    write(&opts.out.join("sw.js"), sw.as_bytes())?;
    println!(
        "bundle: {} ({} files, version {version})",
        opts.out.display(),
        files.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_a_page_without_dev_files() {
        let dir = std::env::temp_dir().join(format!("mb-bundle-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let web = dir.join("web");
        let index = "<head>\n<meta csp>\n<!--build:version-->\n<link manifest>\n</head><body>\n\
                     <script src=\"./guard.js\"></script>\n\
                     <script src=\"./pkg/mb_web.js\"></script>\n<script src=\"./main.js\"></script>\n\
                     </body>";
        write(&web.join("index.html"), index.as_bytes()).unwrap();
        for name in STATIC_FILES {
            write(
                &web.join(name),
                format!("// {name}\r\nlet a = 1;\r\n").as_bytes(),
            )
            .unwrap();
        }
        write(
            &web.join("sw.js"),
            b"const VERSION = \"__MB_VERSION__\";\nconst FILES = __MB_FILES__;\n",
        )
        .unwrap();
        write(&web.join("dev/dev-disc.js"), b"// dev only").unwrap();
        write(&web.join("pkg/mb_web.js"), b"let wasm_bindgen;\n").unwrap();
        write(&web.join("pkg/mb_web_bg.wasm"), &[0, 97, 115, 109]).unwrap();

        let out = dir.join("dist");
        let options = || Options {
            web: web.clone(),
            pkg: web.join("pkg"),
            out: out.clone(),
        };
        run(options()).unwrap();
        let listed = |d: &Path| -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(d)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            listed(&out),
            [
                "guard.js",
                "icon-192.png",
                "icon-512.png",
                "icon.svg",
                "index.html",
                "main.js",
                "manifest.webmanifest",
                "pkg",
                "sw.js",
                "worker.js"
            ]
        );
        assert_eq!(listed(&out.join("pkg")), ["mb_web.js", "mb_web_bg.wasm"]);
        let page = fs::read_to_string(out.join("index.html")).unwrap();
        assert!(page.contains("<meta csp>") && !page.contains("build:"));
        let version = page
            .split("name=\"mb-version\" content=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("the page carries the build version")
            .to_string();
        assert_eq!(version.len(), 16);
        assert!(page.contains(&format!("src=\"./main.js?v={version}\"")));
        assert!(page.contains(&format!("src=\"./pkg/mb_web.js?v={version}\"")));
        let sw = fs::read_to_string(out.join("sw.js")).unwrap();
        assert!(sw.contains("\"pkg/mb_web_bg.wasm\"") && !sw.contains("__MB_"));
        assert!(
            sw.contains(&format!("\"{version}\"")),
            "the cache is named by the same version"
        );
        assert!(!sw.contains("dev"));
        // Line ends are normalised: the same bundle wherever it is built.
        assert!(
            !fs::read_to_string(out.join("main.js"))
                .unwrap()
                .contains('\r')
        );
        // A second run starts from a clean directory and names the same files the same way; a changed
        // file changes the version.
        write(&out.join("stale.txt"), b"x").unwrap();
        run(options()).unwrap();
        assert!(!out.join("stale.txt").exists());
        assert!(
            fs::read_to_string(out.join("index.html"))
                .unwrap()
                .contains(&version)
        );
        write(&web.join("main.js"), b"let a = 2;\n").unwrap();
        run(options()).unwrap();
        assert!(
            !fs::read_to_string(out.join("index.html"))
                .unwrap()
                .contains(&version)
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
