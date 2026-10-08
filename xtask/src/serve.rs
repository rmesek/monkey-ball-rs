//! `xtask serve-web`: a static file server for testing the browser build locally. Standard library only, bound
//! to the loopback interface, no caching. It serves the `web/` directory and, when started with `--disc`, the
//! developer's own disc image at `/dev-disc` (with HTTP range support) so the page can be exercised without a
//! file dialog (`http://localhost:<port>/?dev-disc=1`). Nothing from the disc is copied anywhere.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 8080;
const DEV_DISC_ROUTE: &str = "/dev-disc";

pub struct Options {
    pub root: PathBuf,
    pub port: u16,
    pub disc: Option<PathBuf>,
}

/// Parse `[port] [--disc <path>] [--root <dir>]`.
pub fn parse_args(
    args: impl Iterator<Item = String>,
    default_root: PathBuf,
) -> Result<Options, String> {
    let mut opts = Options {
        root: default_root,
        port: DEFAULT_PORT,
        disc: None,
    };
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--disc" => opts.disc = Some(args.next().ok_or("--disc needs a path")?.into()),
            "--root" => opts.root = args.next().ok_or("--root needs a directory")?.into(),
            port => {
                opts.port = port
                    .parse()
                    .map_err(|_| format!("not a port number: {port}"))?
            }
        }
    }
    Ok(opts)
}

pub fn run(opts: Options) -> Result<(), String> {
    if !opts.root.join("index.html").is_file() {
        return Err(format!("{} has no index.html", opts.root.display()));
    }
    if !opts.root.join("pkg").is_dir() {
        eprintln!(
            "note: {} does not exist yet; run scripts/build-web.ps1 (or .sh) first",
            opts.root.join("pkg").display()
        );
    }
    if let Some(disc) = &opts.disc {
        if !disc.is_file() {
            return Err(format!("disc image not found: {}", disc.display()));
        }
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, opts.port))
        .map_err(|e| format!("cannot listen on 127.0.0.1:{}: {e}", opts.port))?;
    println!(
        "serving {} at http://localhost:{}/",
        opts.root.display(),
        opts.port
    );
    if let Some(disc) = &opts.disc {
        println!(
            "developer disc {} at http://localhost:{}/?dev-disc=1",
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
    let path = if request.path == DEV_DISC_ROUTE {
        opts.disc.clone()
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
            (o.port, o.disc, o.root),
            (9000, Some("a.rvz".into()), PathBuf::from("web"))
        );
        let o = parse_args(args(&[]), "web".into()).unwrap();
        assert_eq!((o.port, o.disc), (DEFAULT_PORT, None));
        assert!(parse_args(args(&["--disc"]), "web".into()).is_err());
        assert!(parse_args(args(&["port"]), "web".into()).is_err());
    }
}
