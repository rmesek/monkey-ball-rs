//! `cargo web`: the built site on this machine, for trying it. A few lines of static file server so that
//! one cargo command is enough on every platform; any other static server does the same job (the site is
//! made for `python -m http.server`, and the browser check serves it with that).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "wasm" => "application/wasm",
        "webmanifest" | "json" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        _ => "application/octet-stream",
    }
}

/// The file a request line asks for, inside `root`; `None` for anything else.
fn target(root: &Path, request_line: &str) -> Option<PathBuf> {
    let mut parts = request_line.split_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    let path = parts.next()?.split(['?', '#']).next()?;
    let relative = Path::new(path.trim_start_matches('/'));
    // Nothing above the site: only plain names.
    if !relative
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let file = root.join(relative);
    Some(if file.is_dir() {
        file.join("index.html")
    } else {
        file
    })
}

fn answer(root: &Path, mut stream: TcpStream) -> std::io::Result<()> {
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let body = target(root, &line).and_then(|file| Some((std::fs::read(&file).ok()?, file)));
    match body {
        Some((bytes, file)) => {
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
                content_type(&file),
                bytes.len()
            )?;
            stream.write_all(&bytes)
        }
        None => stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
    }
}

/// Serve `root` on this machine only, until the process is stopped.
pub fn run(root: &Path, port: u16) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("port {port} cannot be used ({e}); try --port with another"))?;
    println!(
        "serving {} on http://localhost:{port}  (Ctrl+C stops)",
        root.display()
    );
    for stream in listener.incoming().flatten() {
        // A connection the browser gave up on is not an error of the server.
        let _ = answer(root, stream);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn serves_files_of_the_site_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("mb-serve-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("site/pkg")).unwrap();
        std::fs::write(dir.join("site/index.html"), "<p>page</p>").unwrap();
        std::fs::write(dir.join("site/pkg/a.wasm"), [0, 97, 115, 109]).unwrap();
        std::fs::write(dir.join("secret.txt"), "no").unwrap();
        let root = dir.join("site");

        assert_eq!(
            target(&root, "GET / HTTP/1.1"),
            Some(root.join("index.html"))
        );
        assert_eq!(
            target(&root, "GET /pkg/a.wasm?v=12 HTTP/1.1"),
            Some(root.join("pkg/a.wasm"))
        );
        assert_eq!(target(&root, "GET /../secret.txt HTTP/1.1"), None);
        assert_eq!(target(&root, "POST / HTTP/1.1"), None);
        assert_eq!(content_type(Path::new("x/a.wasm")), "application/wasm");

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = root.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = answer(&served, stream);
            }
        });
        let get = |path: &str| {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut out = Vec::new();
            s.read_to_end(&mut out).unwrap();
            String::from_utf8_lossy(&out).into_owned()
        };
        let page = get("/");
        assert!(
            page.starts_with("HTTP/1.1 200") && page.ends_with("<p>page</p>"),
            "{page}"
        );
        assert!(get("/pkg/a.wasm").contains("Content-Type: application/wasm"));
        assert!(get("/../secret.txt").starts_with("HTTP/1.1 404"));
        assert!(get("/missing.js").starts_with("HTTP/1.1 404"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
