//! `xtask serve-web`: a static file server for trying the browser build. A DEVELOPER TOOL: the game itself
//! needs no server (the bundle is static files; see `docs/WEB.md` for hosting, installing and the single-file
//! build). Standard library only, no caching.
//!
//! * By default it listens on the loopback interface only and serves `web/dist/` (what `scripts/build-web`
//!   produces) at `http://localhost:8080/`.
//! * `--lan` listens on every interface, so a phone on the same network can open the page. The page is then
//!   reachable by everyone on that network, and it is plain http: to the phone's browser that is not a
//!   secure context, so there is no private file storage, no service worker and no WebGPU there. The page
//!   handles that (it plays from the picked image for the session, on WebGL2).
//! * `--disc <image>` adds a developer page at `/dev/` that feeds the developer's own disc image to the
//!   page through its file input, for automation that cannot drive a file dialog. The image is served to
//!   loopback clients only, and `--disc` cannot be combined with `--lan`: a disc image is never exposed to a
//!   network.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 8080;
const DEV_DISC_ROUTE: &str = "/dev-disc";
/// The developer page (the bundle's page plus `web/dev/dev-disc.js`) and its script.
const DEV_PAGE_ROUTE: &str = "/dev/";
const DEV_SCRIPT_ROUTE: &str = "/dev/dev-disc.js";

pub struct Options {
    /// The bundle to serve (`web/dist`).
    pub root: PathBuf,
    /// Developer-only files (`web/dev`), served under `/dev/` with `--disc`.
    pub dev: PathBuf,
    pub port: u16,
    pub disc: Option<PathBuf>,
    /// Listen on all interfaces instead of loopback.
    pub lan: bool,
}

/// Parse `[port] [--root <dir>] [--lan] [--disc <path>]`. `web` is the workspace's `web/` directory.
pub fn parse_args(args: impl Iterator<Item = String>, web: PathBuf) -> Result<Options, String> {
    let mut opts = Options {
        root: web.join("dist"),
        dev: web.join("dev"),
        port: DEFAULT_PORT,
        disc: None,
        lan: false,
    };
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--disc" => opts.disc = Some(args.next().ok_or("--disc needs a path")?.into()),
            "--root" => opts.root = args.next().ok_or("--root needs a directory")?.into(),
            "--lan" => opts.lan = true,
            port => {
                opts.port = port
                    .parse()
                    .map_err(|_| format!("not a port number: {port}"))?
            }
        }
    }
    if opts.lan && opts.disc.is_some() {
        return Err(
            "--disc cannot be combined with --lan: a disc image is never served to a network \
             (on the phone, pick the image with the page's own button)"
                .into(),
        );
    }
    Ok(opts)
}

/// This machine's address on the local network, found without sending anything: the address a UDP socket
/// would use towards a public address (192.0.2.1 is reserved for documentation and never contacted).
fn lan_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

pub fn run(opts: Options) -> Result<(), String> {
    if !opts.root.join("index.html").is_file() {
        return Err(format!(
            "{} has no index.html: build the bundle first with scripts/build-web.ps1 (or .sh)",
            opts.root.display()
        ));
    }
    if let Some(disc) = &opts.disc {
        if !disc.is_file() {
            return Err(format!("disc image not found: {}", disc.display()));
        }
    }
    let bind = if opts.lan {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::LOCALHOST
    };
    let listener = TcpListener::bind((bind, opts.port))
        .map_err(|e| format!("cannot listen on {bind}:{}: {e}", opts.port))?;
    println!(
        "serving {} at http://localhost:{}/  (developer tool; the game needs no server)",
        opts.root.display(),
        opts.port
    );
    if opts.lan {
        println!("WARNING: --lan makes this page reachable by every device on the local network.");
        match lan_address() {
            Some(ip) => println!(
                "on a phone in the same network open http://{ip}:{}/",
                opts.port
            ),
            None => println!(
                "no local network address found; look it up in the system's network settings"
            ),
        }
        println!(
            "note: plain http on a network address is not a secure context: no on-device storage, no service \
             worker, no WebGPU there. The page then plays from the picked image for the session (WebGL2).\n\
             With a USB-connected Android device, `adb reverse tcp:{0} tcp:{0}` and http://localhost:{0}/ on \
             the phone is a secure context and needs no --lan.",
            opts.port
        );
    }
    if let Some(disc) = &opts.disc {
        println!(
            "developer page with disc {} at http://localhost:{}{DEV_PAGE_ROUTE} (loopback only)",
            disc.display(),
            opts.port
        );
    }
    println!("press Ctrl+C to stop");
    let opts = Arc::new(opts);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let opts = opts.clone();
        std::thread::spawn(move || {
            // A dropped connection (tab closed mid-download) is not worth reporting.
            let _ = handle(stream, &opts);
        });
    }
    Ok(())
}

/// The bundle's page as the developer page: relative paths resolve against the bundle's root and the
/// developer script is added. Built in memory; the bundle itself never contains it.
fn dev_page(index: &str) -> String {
    index
        .replacen("<head>", "<head>\n<base href=\"/\">", 1)
        .replacen(
            "</body>",
            &format!("  <script src=\"{DEV_SCRIPT_ROUTE}\"></script>\n</body>"),
            1,
        )
}

#[derive(Debug, PartialEq, Eq)]
struct Request {
    method: String,
    /// Path without the query string, percent-decoded.
    path: String,
    range: Option<String>,
}

fn read_request(reader: &mut impl BufRead) -> io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.take(8192).read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad request line",
        ));
    };
    let path = percent_decode(target.split(['?', '#']).next().unwrap_or(""));
    let mut range = None;
    loop {
        let mut header = String::new();
        if reader.take(8192).read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("range") {
                range = Some(value.trim().to_string());
            }
        }
    }
    Ok(Some(Request {
        method: method.to_string(),
        path,
        range,
    }))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1), bytes.get(i + 2)) {
            (b'%', Some(&a), Some(&b)) if hex(a).is_some() && hex(b).is_some() => {
                out.push((hex(a).unwrap() * 16 + hex(b).unwrap()) as u8);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Map a URL path to a file under `root`. Only plain path components are accepted, so nothing outside the
/// directory can be reached.
fn resolve(root: &Path, url_path: &str) -> Option<PathBuf> {
    let rel = url_path.trim_start_matches('/');
    if rel.contains('\\') || rel.contains(':') {
        return None;
    }
    let mut path = root.to_path_buf();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if url_path.ends_with('/') || rel.is_empty() {
        path.push("index.html");
    }
    Some(path)
}

fn mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("webmanifest") => "application/manifest+json",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Parse a single `bytes=a-b` / `bytes=a-` / `bytes=-n` range against a body of `len` bytes into an inclusive
/// `(first, last)`. `None` means the header is not a satisfiable single range.
fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let spec = header.strip_prefix("bytes=")?;
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (first, last) = spec.split_once('-')?;
    let (first, last) = match (first.trim(), last.trim()) {
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            (len.checked_sub(n.min(len))?, len - 1)
        }
        (first, "") => (first.parse().ok()?, len - 1),
        (first, last) => (first.parse().ok()?, last.parse::<u64>().ok()?.min(len - 1)),
    };
    (first <= last && first < len).then_some((first, last))
}

fn respond_status(stream: &mut TcpStream, status: &str, extra: &str, body: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n{extra}\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
}

fn handle(stream: TcpStream, opts: &Options) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let local_client = stream.peer_addr().is_ok_and(|a| a.ip().is_loopback());
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut stream = stream;
    let Some(request) = read_request(&mut reader)? else {
        return Ok(());
    };
    let head = request.method == "HEAD";
    if !head && request.method != "GET" {
        return respond_status(
            &mut stream,
            "405 Method Not Allowed",
            "Allow: GET, HEAD\r\n",
            "method not allowed\n",
        );
    }
    // The developer routes exist only with --disc, and only for this machine.
    let dev = opts.disc.is_some() && local_client;
    if dev && request.path == DEV_PAGE_ROUTE {
        let page = dev_page(&std::fs::read_to_string(opts.root.join("index.html"))?);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nConnection: close\r\n\r\n",
            page.len()
        )?;
        if !head {
            stream.write_all(page.as_bytes())?;
        }
        return stream.flush();
    }
    let path = if request.path == DEV_DISC_ROUTE {
        opts.disc.clone().filter(|_| dev)
    } else if request.path == DEV_SCRIPT_ROUTE {
        dev.then(|| opts.dev.join("dev-disc.js"))
    } else {
        resolve(&opts.root, &request.path)
    };
    let file = path
        .as_deref()
        .filter(|p| p.is_file())
        .and_then(|p| File::open(p).ok().map(|f| (p, f)));
    let Some((path, mut file)) = file else {
        return respond_status(&mut stream, "404 Not Found", "", "not found\n");
    };
    let len = file.metadata()?.len();
    let content_type = if request.path == DEV_DISC_ROUTE {
        "application/octet-stream"
    } else {
        mime(path)
    };
    let (status, first, count, range_header) = match request.range.as_deref() {
        None => ("200 OK", 0, len, String::new()),
        Some(range) => match parse_range(range, len) {
            Some((first, last)) => (
                "206 Partial Content",
                first,
                last - first + 1,
                format!("Content-Range: bytes {first}-{last}/{len}\r\n"),
            ),
            None => {
                let extra = format!("Content-Range: bytes */{len}\r\n");
                return respond_status(
                    &mut stream,
                    "416 Range Not Satisfiable",
                    &extra,
                    "bad range\n",
                );
            }
        },
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {count}\r\n{range_header}\
         Accept-Ranges: bytes\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\r\n"
    )?;
    if !head {
        file.seek(SeekFrom::Start(first))?;
        // Large blocks: the default 8 KiB copy buffer makes a 0.5 GB disc image crawl.
        let mut block = vec![0u8; 1 << 20];
        let mut left = count;
        while left > 0 {
            let n = file.read(&mut block[..left.min(1 << 20) as usize])?;
            if n == 0 {
                break;
            }
            stream.write_all(&block[..n])?;
            left -= n as u64;
        }
    }
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requests() {
        let raw = "GET /pkg/mb_web.js?v=1 HTTP/1.1\r\nHost: localhost\r\nrange: bytes=0-9\r\n\r\n";
        let req = read_request(&mut raw.as_bytes()).unwrap().unwrap();
        assert_eq!(
            req,
            Request {
                method: "GET".into(),
                path: "/pkg/mb_web.js".into(),
                range: Some("bytes=0-9".into())
            }
        );
        assert!(read_request(&mut "".as_bytes()).unwrap().is_none());
        assert!(read_request(&mut "nonsense\r\n\r\n".as_bytes()).is_err());
        assert_eq!(percent_decode("/a%20b%2Fc%zz"), "/a b/c%zz");
    }

    #[test]
    fn resolves_only_inside_the_root() {
        let root = Path::new("web");
        assert_eq!(resolve(root, "/"), Some(root.join("index.html")));
        assert_eq!(
            resolve(root, "/pkg/mb_web_bg.wasm"),
            Some(root.join("pkg").join("mb_web_bg.wasm"))
        );
        assert_eq!(resolve(root, "/./main.js"), Some(root.join("main.js")));
        assert_eq!(resolve(root, "/../Cargo.toml"), None);
        assert_eq!(resolve(root, "/pkg/../../Cargo.toml"), None);
        assert_eq!(resolve(root, "/..\\Cargo.toml"), None);
        assert_eq!(resolve(root, "/C:/Windows/win.ini"), None);
        assert_eq!(
            resolve(root, "//server/share"),
            Some(root.join("server").join("share"))
        );
    }

    #[test]
    fn knows_the_types_the_page_needs() {
        assert_eq!(mime(Path::new("pkg/mb_web_bg.wasm")), "application/wasm");
        assert_eq!(mime(Path::new("main.JS")), "text/javascript; charset=utf-8");
        assert_eq!(mime(Path::new("index.html")), "text/html; charset=utf-8");
        assert_eq!(mime(Path::new("thing.rvz")), "application/octet-stream");
    }

    #[test]
    fn the_developer_page_is_the_bundle_page_plus_one_script() {
        let page =
            dev_page("<html><head>\n<title>x</title></head><body>\n<p>hi</p>\n</body></html>");
        assert!(page.contains("<head>\n<base href=\"/\">"));
        assert!(page.contains("<script src=\"/dev/dev-disc.js\"></script>\n</body>"));
        assert_eq!(page.matches("<script").count(), 1);
    }

    #[test]
    fn parses_ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=90-500", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-500", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=9-0", 100), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
        assert_eq!(parse_range("bytes=0-", 0), None);
    }

    #[test]
    fn parses_arguments() {
        let args = |v: &[&str]| {
            v.iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .into_iter()
        };
        let o = parse_args(args(&["9000", "--disc", "a.rvz"]), "web".into()).unwrap();
        assert_eq!(
            (o.port, o.disc, o.root, o.lan),
            (
                9000,
                Some("a.rvz".into()),
                PathBuf::from("web").join("dist"),
                false
            )
        );
        let o = parse_args(args(&[]), "web".into()).unwrap();
        assert_eq!((o.port, o.disc, o.lan), (DEFAULT_PORT, None, false));
        let o = parse_args(args(&["--lan", "--root", "x"]), "web".into()).unwrap();
        assert!(o.lan && o.root.as_os_str() == "x" && o.dev == Path::new("web").join("dev"));
        // A disc image is never offered to the network, in either order.
        for both in [["--lan", "--disc", "a.rvz"], ["--disc", "a.rvz", "--lan"]] {
            let e = parse_args(args(&both), "web".into()).err().unwrap();
            assert!(e.contains("--lan"), "{e}");
        }
        assert!(parse_args(args(&["--disc"]), "web".into()).is_err());
        assert!(parse_args(args(&["port"]), "web".into()).is_err());
    }
}
