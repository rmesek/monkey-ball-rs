//! `xtask bundle-web`: assemble the static web bundle in `web/dist/` from the page sources in `web/` and the
//! wasm-bindgen output. Called by `scripts/build-web.*` after the wasm build; standard library only.
//!
//! Two products:
//! * the **hosted bundle** (always): `index.html`, `main.js`, `worker.js`, `sw.js` (with a version hash and
//!   the precache list filled in), the manifest, the icons and `pkg/`. All paths relative, and nothing else:
//!   only the files named here are copied, whatever else lies in `web/`.
//! * the **single file** (`--single`): `monkey-ball.html` with the scripts inlined as classic scripts, the
//!   wasm module embedded as base64 (gzipped when the build script supplies a `.gz`), the worker source
//!   carried as text and the icon as a data URI. No service worker, no manifest. Its Content-Security-Policy
//!   names the three inline scripts by hash and forbids every connection.

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
pub const SINGLE_FILE: &str = "monkey-ball.html";

pub struct Options {
    /// The page sources (`web/`).
    pub web: PathBuf,
    /// Directory holding `mb_web.js` and `mb_web_bg.wasm` (wasm-bindgen's `--target no-modules` output).
    pub pkg: PathBuf,
    pub out: PathBuf,
    pub single: bool,
    /// Gzip of `mb_web_bg.wasm` for the single file (the build scripts make it with the system's gzip).
    pub wasm_gz: Option<PathBuf>,
}

pub fn parse_args(args: impl Iterator<Item = String>, web: PathBuf) -> Result<Options, String> {
    let mut opts = Options {
        pkg: web.join("pkg"),
        out: web.join("dist"),
        web,
        single: false,
        wasm_gz: None,
    };
    let mut args = args;
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{what} needs a path"));
        match arg.as_str() {
            "--single" => opts.single = true,
            "--pkg" => opts.pkg = value("--pkg")?.into(),
            "--out" => opts.out = value("--out")?.into(),
            "--wasm-gz" => opts.wasm_gz = Some(value("--wasm-gz")?.into()),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(opts)
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A text source with Unix line ends (a checkout on Windows may have converted them; the hashes in the
/// single file's policy are over the text exactly as the browser sees it).
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

/// Replace the block between `<!--build:NAME-->` and `<!--/build:NAME-->` (markers included) with `with`.
fn replace_block(html: &str, name: &str, with: &str) -> Result<String, String> {
    let (open, close) = (
        format!("<!--build:{name}-->"),
        format!("<!--/build:{name}-->"),
    );
    let start = html
        .find(&open)
        .ok_or(format!("index.html has no {open}"))?;
    let end = html[start..]
        .find(&close)
        .ok_or(format!("index.html has no {close}"))?
        + start
        + close.len();
    Ok(format!("{}{}{}", &html[..start], with, &html[end..]))
}

/// The inside of a block (markers removed).
fn block_content(html: &str, name: &str) -> Result<String, String> {
    let (open, close) = (
        format!("<!--build:{name}-->"),
        format!("<!--/build:{name}-->"),
    );
    let start = html
        .find(&open)
        .ok_or(format!("index.html has no {open}"))?
        + open.len();
    let end = html[start..]
        .find(&close)
        .ok_or(format!("index.html has no {close}"))?
        + start;
    Ok(html[start..end].trim_matches('\n').to_string())
}

/// Text that goes inside a `<script>` element must not end it (or open an HTML comment).
fn check_inline(name: &str, text: &str) -> Result<(), String> {
    let lower = text.to_ascii_lowercase();
    for bad in ["</script", "<!--"] {
        if lower.contains(bad) {
            return Err(format!("{name} contains {bad:?} and cannot be inlined"));
        }
    }
    Ok(())
}

pub fn run(opts: Options) -> Result<(), String> {
    let index = read_text(&opts.web.join("index.html"))?;
    let pkg_file = |name: &str| opts.pkg.join(name.trim_start_matches("pkg/"));
    for name in PKG_FILES {
        if !pkg_file(name).is_file() {
            return Err(format!(
                "{} is missing: run scripts/build-web first (it builds the wasm and then calls this)",
                pkg_file(name).display()
            ));
        }
    }
    if opts.out.exists() {
        fs::remove_dir_all(&opts.out).map_err(|e| format!("{}: {e}", opts.out.display()))?;
    }

    // ---- hosted bundle ---------------------------------------------------------------------------------
    let mut hosted = index.clone();
    for name in ["csp", "head", "scripts"] {
        hosted = replace_block(&hosted, name, &block_content(&index, name)?)?;
    }
    let mut hasher = Sha256::new();
    let mut put = |name: &str, bytes: &[u8]| -> Result<(), String> {
        hasher.update(name.as_bytes());
        hasher.update(&(bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
        write(&opts.out.join(name), bytes)
    };
    // The page itself is hashed before the version is stamped into it, and written afterwards.
    put("index.html", hosted.as_bytes())?;
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
    let version = hex(&hasher.finish()[..8]);
    // Every reference to a bundle file carries the version, so a server that sends no cache headers
    // (python -m http.server) cannot pair an old script with a new engine.
    let stamp = format!("<meta name=\"mb-version\" content=\"{version}\">");
    let mut hosted = replace_block(&hosted, "version", &stamp)?;
    for file in ["./guard.js", "./pkg/mb_web.js", "./main.js"] {
        let reference = format!("src=\"{file}\"");
        if !hosted.contains(&reference) {
            return Err(format!("index.html no longer loads {file}"));
        }
        hosted = hosted.replace(&reference, &format!("src=\"{file}?v={version}\""));
    }
    write(&opts.out.join("index.html"), hosted.as_bytes())?;
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

    // ---- single file ------------------------------------------------------------------------------------
    if opts.single {
        let guard = read_text(&opts.web.join("guard.js"))?;
        let glue = read_text(&pkg_file("pkg/mb_web.js"))?;
        let main = read_text(&opts.web.join("main.js"))?;
        let worker = read_text(&opts.web.join("worker.js"))?;
        let icon = read_text(&opts.web.join("icon.svg"))?;
        for (name, text) in [
            ("guard.js", &guard),
            ("mb_web.js", &glue),
            ("main.js", &main),
            ("worker.js", &worker),
        ] {
            check_inline(name, text)?;
        }
        let (wasm, encoding) = match &opts.wasm_gz {
            Some(gz) => (read(gz)?, "gzip"),
            None => (read(&pkg_file("pkg/mb_web_bg.wasm"))?, "none"),
        };
        // The browser hashes the script text exactly as it stands between the tags.
        let guard_block = format!("\n{guard}\n");
        let glue_block = format!("\n{glue}\n");
        let main_block = format!("\n{main}\n");
        let csp = format!(
            "<meta http-equiv=\"Content-Security-Policy\"\n      content=\"default-src 'none'; \
             script-src 'sha256-{}' 'sha256-{}' 'sha256-{}' 'wasm-unsafe-eval'; style-src 'unsafe-inline'; \
             img-src data: blob:; worker-src blob:; child-src blob:; connect-src 'none'; \
             form-action 'none'; base-uri 'none'\">",
            base64(&Sha256::digest(guard_block.as_bytes())),
            base64(&Sha256::digest(glue_block.as_bytes())),
            base64(&Sha256::digest(main_block.as_bytes())),
        );
        let head = format!(
            "<link rel=\"icon\" href=\"data:image/svg+xml;base64,{}\">",
            base64(icon.as_bytes())
        );
        let scripts = format!(
            "  <script type=\"application/octet-stream\" id=\"mb-wasm\" data-encoding=\"{encoding}\">{}</script>\n  \
             <script type=\"text/plain\" id=\"mb-worker\">\n{worker}\n</script>\n  \
             <script>{guard_block}</script>\n  \
             <script id=\"mb-glue\">{glue_block}</script>\n  <script>{main_block}</script>",
            base64(&wasm)
        );
        let mut single = replace_block(&index, "csp", &csp)?;
        single = replace_block(&single, "version", "")?;
        single = replace_block(&single, "head", &head)?;
        single = replace_block(&single, "scripts", &scripts)?;
        let path = opts.out.join(SINGLE_FILE);
        write(&path, single.as_bytes())?;
        println!(
            "single file: {} ({:.1} MB, wasm {encoding})",
            path.display(),
            single.len() as f64 / 1e6
        );
    }
    Ok(())
}

// ---- SHA-256 and base64 (the standard library has neither) -------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    len: u64,
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            block: [0; 64],
            filled: 0,
            len: 0,
        }
    }

    fn compress(&mut self) {
        let mut w = [0u32; 64];
        for (i, word) in self.block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            (h, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        while !data.is_empty() {
            let n = (64 - self.filled).min(data.len());
            self.block[self.filled..self.filled + n].copy_from_slice(&data[..n]);
            self.filled += n;
            data = &data[n..];
            if self.filled == 64 {
                self.compress();
                self.filled = 0;
            }
        }
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.len * 8;
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.block[56..].copy_from_slice(&bits.to_be_bytes());
        self.compress();
        let mut out = [0u8; 32];
        for (o, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *o = s.to_be_bytes();
        }
        out
    }

    pub fn digest(data: &[u8]) -> [u8; 32] {
        let mut h = Self::new();
        h.update(data);
        h.finish()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(
            hex(&Sha256::digest(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&Sha256::digest(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // Fed in pieces across the block boundary.
        let data = vec![b'a'; 1000];
        let mut h = Sha256::new();
        for piece in data.chunks(37) {
            h.update(piece);
        }
        assert_eq!(h.finish(), Sha256::digest(&data));
        assert_eq!(
            hex(&Sha256::digest(&data)),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn blocks_are_replaced_and_inline_text_is_checked() {
        let html = "a\n<!--build:x-->\nold\n<!--/build:x-->\nb";
        assert_eq!(replace_block(html, "x", "NEW").unwrap(), "a\nNEW\nb");
        assert_eq!(block_content(html, "x").unwrap(), "old");
        assert!(replace_block(html, "y", "").is_err());
        assert!(check_inline("a.js", "let s = '<\\/script>';").is_ok());
        assert!(check_inline("a.js", "let s = '</SCRIPT>';").is_err());
        assert!(check_inline("a.js", "x <!-- y").is_err());
    }

    #[test]
    fn bundles_a_page_without_dev_files() {
        let dir = std::env::temp_dir().join(format!("mb-bundle-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let web = dir.join("web");
        let index = "<head>\n<!--build:csp-->\n<meta csp>\n<!--/build:csp-->\n\
                     <!--build:version--><!--/build:version-->\n<!--build:head-->\n<link manifest>\n\
                     <!--/build:head-->\n</head><body>\n<!--build:scripts-->\n\
                     <script src=\"./guard.js\"></script>\n\
                     <script src=\"./pkg/mb_web.js\"></script>\n<script src=\"./main.js\"></script>\n\
                     <!--/build:scripts-->\n</body>";
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

        let mut opts = parse_args(["--single".to_string()].into_iter(), web.clone()).unwrap();
        opts.out = dir.join("dist");
        let out = opts.out.clone();
        run(opts).unwrap();
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
                "monkey-ball.html",
                "pkg",
                "sw.js",
                "worker.js"
            ]
        );
        assert_eq!(listed(&out.join("pkg")), ["mb_web.js", "mb_web_bg.wasm"]);
        let hosted = fs::read_to_string(out.join("index.html")).unwrap();
        assert!(hosted.contains("<meta csp>") && !hosted.contains("build:"));
        let version = hosted
            .split("name=\"mb-version\" content=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("the page carries the build version");
        assert_eq!(version.len(), 16);
        assert!(hosted.contains(&format!("src=\"./main.js?v={version}\"")));
        assert!(hosted.contains(&format!("src=\"./pkg/mb_web.js?v={version}\"")));
        let sw = fs::read_to_string(out.join("sw.js")).unwrap();
        assert!(sw.contains("\"pkg/mb_web_bg.wasm\"") && !sw.contains("__MB_"));
        assert!(
            sw.contains(&format!("\"{version}\"")),
            "the cache is named by the same version"
        );
        assert!(!sw.contains("dev"));
        // Line ends are normalised, so the hash in the policy is over what the browser parses.
        assert!(!fs::read_to_string(out.join("main.js"))
            .unwrap()
            .contains('\r'));
        let single = fs::read_to_string(out.join(SINGLE_FILE)).unwrap();
        let main_block = "\n// main.js\nlet a = 1;\n\n";
        assert!(single.contains(&format!("<script>{main_block}</script>")));
        assert!(single.contains(&format!(
            "'sha256-{}'",
            base64(&Sha256::digest(main_block.as_bytes()))
        )));
        assert!(single.contains("connect-src 'none'") && single.contains("id=\"mb-wasm\""));
        assert!(single.contains("data-encoding=\"none\">AGFzbQ==</script>"));
        assert!(!single.contains("manifest") && !single.contains("sw.js"));
        assert!(!single.contains("mb-version") && !single.contains("build:"));
        // A second run starts from a clean directory.
        write(&out.join("stale.txt"), b"x").unwrap();
        let mut opts = parse_args(std::iter::empty(), web).unwrap();
        opts.out = out.clone();
        run(opts).unwrap();
        assert!(!out.join("stale.txt").exists() && !out.join(SINGLE_FILE).exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
