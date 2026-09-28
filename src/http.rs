//! A minimal HTTPS `GET` client.
//!
//! TLS is `rustls` (never write your own); everything around it is here, and
//! only covers what tachobar needs: fetching a few fixed `https://` URLs,
//! following redirects, with time and size limits. It honours `HTTPS_PROXY` /
//! `NO_PROXY` (via `CONNECT`) and `SSL_CERT_FILE` (extra trusted CAs, for
//! TLS-inspecting proxies), like most command-line tools.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

/// Time allowed for a whole request, redirects included.
const TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY: usize = 64 * 1024 * 1024;
const MAX_HEAD: usize = 64 * 1024;
const MAX_REDIRECTS: usize = 5;

/// GET an `https://` URL and return the body as text.
pub fn get(url: &str) -> Result<String, String> {
    fetch_text(url).map_err(|e| {
        let hint = if e.contains("UnknownIssuer") {
            " (behind a TLS-inspecting proxy? point SSL_CERT_FILE at your CA bundle)"
        } else {
            ""
        };
        format!("{url}: {e}{hint}")
    })
}

/// `host:port` of the proxy that `HTTPS_PROXY` / `ALL_PROXY` select, if any.
pub fn proxy_in_use() -> Option<String> {
    let env = |k: &str| std::env::var(k).ok();
    let p = proxy_for("", &env).ok().flatten()?;
    Some(format!("{}:{}", p.host, p.port))
}

fn fetch_text(url: &str) -> Result<String, String> {
    let deadline = Instant::now() + TIMEOUT;
    let tls = tls_config()?;
    let mut url = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let target = parse_https_url(&url)?;
        match fetch(&target, &tls, deadline)? {
            Reply::Body(bytes) => {
                return String::from_utf8(bytes).map_err(|_| "response is not UTF-8".to_string())
            }
            Reply::Redirect(location) => url = resolve_redirect(&target, &location)?,
        }
    }
    Err("too many redirects".into())
}

// ---------------------------------------------------------------- URLs

#[derive(Debug, PartialEq)]
struct Url {
    host: String,
    port: u16,
    /// Path and query, always starting with `/`.
    path: String,
}

impl Url {
    /// `host`, or `host:port` for a non-default port (the `Host` header).
    fn authority(&self) -> String {
        if self.port == 443 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

fn parse_https_url(url: &str) -> Result<Url, String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or("only https:// URLs are supported")?;
    let rest = rest.split('#').next().unwrap_or(rest);
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| "invalid port in URL")?),
        None => (authority, 443),
    };
    let host_ok = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    // No spaces or control characters: they would break out of the request line.
    let path_ok = path.bytes().all(|b| (0x21..0x7f).contains(&b));
    if !host_ok || !path_ok {
        return Err("invalid URL".into());
    }
    Ok(Url {
        host: host.to_ascii_lowercase(),
        port,
        path: path.to_string(),
    })
}

fn resolve_redirect(from: &Url, location: &str) -> Result<String, String> {
    if location.starts_with("https://") {
        Ok(location.to_string())
    } else if location.starts_with('/') {
        Ok(format!("https://{}{location}", from.authority()))
    } else {
        Err(format!("unsupported redirect to {location:?}"))
    }
}

// ---------------------------------------------------------------- connecting

/// A TCP stream whose every read and write is bounded by one overall deadline.
struct Timed {
    tcp: TcpStream,
    deadline: Instant,
}

impl Timed {
    fn arm(&self) -> io::Result<()> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "request timed out"));
        }
        self.tcp.set_read_timeout(Some(left))?;
        self.tcp.set_write_timeout(Some(left))
    }
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.arm()?;
        self.tcp.read(buf)
    }
}

impl Write for Timed {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.arm()?;
        self.tcp.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}

fn tls_config() -> Result<Arc<ClientConfig>, String> {
    let mut roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    // Extra CAs (a corporate TLS-inspecting proxy). An unreadable file is
    // ignored, so a stale variable can't break downloads.
    if let Some(file) = std::env::var_os("SSL_CERT_FILE").filter(|v| !v.is_empty()) {
        if let Ok(certs) = CertificateDer::pem_file_iter(file) {
            roots.add_parsable_certificates(certs.filter_map(Result::ok));
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

fn fetch(target: &Url, tls: &Arc<ClientConfig>, deadline: Instant) -> Result<Reply, String> {
    let sock = connect(target, deadline)?;
    let name = ServerName::try_from(target.host.clone()).map_err(|e| e.to_string())?;
    let conn = ClientConnection::new(tls.clone(), name).map_err(|e| e.to_string())?;
    exchange(StreamOwned::new(conn, sock), target)
}

fn connect(target: &Url, deadline: Instant) -> Result<Timed, String> {
    let env = |k: &str| std::env::var(k).ok();
    let proxy = proxy_for(&target.host, &env)?;
    let (host, port) = match &proxy {
        Some(p) => (p.host.as_str(), p.port),
        None => (target.host.as_str(), target.port),
    };
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {host}: {e}"))?;
    let mut last = None;
    for addr in addrs {
        let left = deadline
            .saturating_duration_since(Instant::now())
            .min(CONNECT_TIMEOUT);
        match TcpStream::connect_timeout(&addr, left) {
            Ok(tcp) => {
                let _ = tcp.set_nodelay(true);
                let mut sock = Timed { tcp, deadline };
                if let Some(p) = &proxy {
                    tunnel(&mut sock, target, p)?;
                }
                return Ok(sock);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) => format!("cannot connect to {host}:{port}: {e}"),
        None => format!("no address for {host}"),
    })
}

// ---------------------------------------------------------------- proxy

#[derive(Debug, PartialEq)]
struct Proxy {
    host: String,
    port: u16,
    /// `user:password`, already percent-decoded.
    auth: Option<String>,
}

/// The proxy for `host`, from the usual environment variables.
fn proxy_for(host: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Option<Proxy>, String> {
    let first = |names: &[&str]| {
        names
            .iter()
            .filter_map(|n| env(n))
            .find(|v| !v.trim().is_empty())
    };
    let Some(raw) = first(&["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]) else {
        return Ok(None);
    };
    if first(&["NO_PROXY", "no_proxy"]).is_some_and(|list| no_proxy_matches(&list, host)) {
        return Ok(None);
    }
    parse_proxy(raw.trim()).map(Some)
}

/// `NO_PROXY` semantics: a comma-separated list of host names; `example.com`
/// and `.example.com` both match the host and its subdomains, `*` matches all.
fn no_proxy_matches(list: &str, host: &str) -> bool {
    list.split(',').map(str::trim).any(|entry| {
        let entry = entry.split(':').next().unwrap_or("");
        let entry = entry.trim_start_matches("*").trim_start_matches('.');
        entry.is_empty() && list.trim() == "*"
            || !entry.is_empty()
                && !entry.contains('/')
                && (host == entry || host.ends_with(&format!(".{entry}")))
    })
}

fn parse_proxy(raw: &str) -> Result<Proxy, String> {
    // Never echo the URL back: it may contain a password.
    let bad = || "invalid HTTPS_PROXY (expected http://[user:password@]host:port)".to_string();
    let rest = match raw.split_once("://") {
        Some(("http", rest)) => rest,
        Some(_) => return Err(bad()),
        None => raw,
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| bad())?),
        None => (hostport, 80),
    };
    let host = host.trim_matches(['[', ']']);
    if host.is_empty() {
        return Err(bad());
    }
    Ok(Proxy {
        host: host.to_string(),
        port,
        auth: userinfo.map(percent_decode),
    })
}

/// Ask the proxy to open a tunnel to the target, then leave the socket ready
/// for the TLS handshake.
fn tunnel<S: Read + Write>(s: &mut S, target: &Url, proxy: &Proxy) -> Result<(), String> {
    let dest = format!("{}:{}", target.host, target.port);
    let mut req = format!("CONNECT {dest} HTTP/1.1\r\nHost: {dest}\r\n");
    if let Some(auth) = &proxy.auth {
        req += &format!("Proxy-Authorization: Basic {}\r\n", base64(auth.as_bytes()));
    }
    req += "\r\n";
    s.write_all(req.as_bytes()).map_err(ioerr)?;
    // One byte at a time, so none of the TLS handshake is swallowed.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 8192 {
            return Err("proxy reply is too large".into());
        }
        if s.read(&mut byte).map_err(ioerr)? == 0 {
            return Err("proxy closed the connection".into());
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head);
    let line = text.lines().next().unwrap_or("");
    let ok = line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .is_some_and(|c| (200..300).contains(&c));
    if ok {
        Ok(())
    } else {
        Err(format!("proxy refused the connection: {line}"))
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |x: u8| (x as char).to_digit(16);
        match (
            b[i],
            b.get(i + 1).and_then(|&x| hex(x)),
            b.get(i + 2).and_then(|&x| hex(x)),
        ) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
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

fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, &b| n << 8 | b as u32) << (8 * (3 - chunk.len()));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ---------------------------------------------------------------- HTTP/1.1

enum Reply {
    Body(Vec<u8>),
    Redirect(String),
}

struct Head {
    status: u16,
    content_length: Option<u64>,
    chunked: bool,
    location: Option<String>,
}

fn ioerr(e: io::Error) -> String {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        "connection closed before the response was complete".into()
    } else {
        e.to_string()
    }
}

/// Send the request and read the reply over an already connected stream.
fn exchange<S: Read + Write>(mut s: S, target: &Url) -> Result<Reply, String> {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: tachobar/{}\r\nAccept: */*\r\n\
         Accept-Encoding: identity\r\nConnection: close\r\n\r\n",
        target.path,
        target.authority(),
        env!("CARGO_PKG_VERSION")
    );
    s.write_all(request.as_bytes()).map_err(ioerr)?;
    s.flush().map_err(ioerr)?;
    read_reply(&mut BufReader::new(s), MAX_BODY)
}

fn read_reply<R: BufRead>(r: &mut R, max_body: usize) -> Result<Reply, String> {
    let head = loop {
        let head = read_head(r)?;
        // Skip interim replies such as 100 Continue and 103 Early Hints.
        if (100..200).contains(&head.status) && head.status != 101 {
            continue;
        }
        break head;
    };
    match head.status {
        200 => read_body(r, &head, max_body).map(Reply::Body),
        301 | 302 | 303 | 307 | 308 => head
            .location
            .map(Reply::Redirect)
            .ok_or_else(|| "redirect without a Location".to_string()),
        status => Err(format!("HTTP status {status}")),
    }
}

/// One `\n`-terminated line, without its line ending, counted against `budget`.
fn read_line<R: BufRead>(r: &mut R, budget: &mut usize) -> Result<String, String> {
    let mut line = Vec::new();
    let limit = *budget as u64 + 1;
    r.by_ref()
        .take(limit)
        .read_until(b'\n', &mut line)
        .map_err(ioerr)?;
    if line.len() > *budget {
        return Err("response headers are too large".into());
    }
    if !line.ends_with(b"\n") {
        return Err("connection closed before the response was complete".into());
    }
    *budget -= line.len();
    let text = String::from_utf8_lossy(&line);
    Ok(text.trim_end_matches(['\r', '\n']).to_string())
}

fn read_head<R: BufRead>(r: &mut R) -> Result<Head, String> {
    let mut budget = MAX_HEAD;
    let status_line = read_line(r, &mut budget)?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or("");
    let status = parts.next().and_then(|s| s.parse::<u16>().ok());
    let (true, Some(status)) = (version.starts_with("HTTP/1."), status) else {
        return Err("not an HTTP/1.x reply".into());
    };
    let mut head = Head {
        status,
        content_length: None,
        chunked: false,
        location: None,
    };
    loop {
        let line = read_line(r, &mut budget)?;
        if line.is_empty() {
            return Ok(head);
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err("malformed header line".into());
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => {
                let n = value.parse::<u64>().map_err(|_| "invalid Content-Length")?;
                if head.content_length.is_some_and(|old| old != n) {
                    return Err("conflicting Content-Length headers".into());
                }
                head.content_length = Some(n);
            }
            "transfer-encoding" => {
                if !value.eq_ignore_ascii_case("chunked") {
                    return Err(format!("unsupported Transfer-Encoding {value:?}"));
                }
                head.chunked = true;
            }
            "content-encoding" if !value.eq_ignore_ascii_case("identity") => {
                return Err(format!("unexpected Content-Encoding {value:?}"));
            }
            "location" => head.location = Some(value.to_string()),
            _ => {}
        }
    }
}

fn read_body<R: BufRead>(r: &mut R, head: &Head, max: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    if head.chunked {
        loop {
            let mut budget = 1024;
            let line = read_line(r, &mut budget)?;
            let size = line.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size, 16).map_err(|_| "invalid chunk size")?;
            if size == 0 {
                // Skip any trailers up to the blank line.
                let mut budget = MAX_HEAD;
                while !read_line(r, &mut budget)?.is_empty() {}
                return Ok(body);
            }
            if size > max - body.len().min(max) {
                return Err("response is too large".into());
            }
            let got = r
                .by_ref()
                .take(size as u64)
                .read_to_end(&mut body)
                .map_err(ioerr)?;
            let mut end = [0u8; 2];
            r.read_exact(&mut end).map_err(ioerr)?;
            if got != size || &end != b"\r\n" {
                return Err("malformed chunked body".into());
            }
        }
    }
    match head.content_length {
        Some(n) if n > max as u64 => Err("response is too large".into()),
        Some(n) => {
            let got = r.by_ref().take(n).read_to_end(&mut body).map_err(ioerr)?;
            if got as u64 == n {
                Ok(body)
            } else {
                Err("connection closed before the response was complete".into())
            }
        }
        // No length: the body ends when the server closes the connection.
        // TLS reports a truncated stream as an error, so this can't be cut short.
        None => {
            r.by_ref()
                .take(max as u64 + 1)
                .read_to_end(&mut body)
                .map_err(ioerr)?;
            if body.len() > max {
                Err("response is too large".into())
            } else {
                Ok(body)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(bytes: &[u8]) -> Result<Reply, String> {
        read_reply(&mut &bytes[..], 1000)
    }

    fn body(bytes: &[u8]) -> String {
        match reply(bytes) {
            Ok(Reply::Body(b)) => String::from_utf8(b).unwrap(),
            Ok(Reply::Redirect(l)) => panic!("unexpected redirect to {l}"),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    fn err(bytes: &[u8]) -> String {
        match reply(bytes) {
            Err(e) => e,
            Ok(_) => panic!("expected an error"),
        }
    }

    #[test]
    fn parses_urls() {
        let u = parse_https_url("https://Example.com/a/b?c=d#frag").unwrap();
        assert_eq!(u.host, "example.com");
        assert_eq!((u.port, u.path.as_str()), (443, "/a/b?c=d"));
        let u = parse_https_url("https://h.example:8443").unwrap();
        assert_eq!((u.port, u.path.as_str()), (8443, "/"));
        assert_eq!(u.authority(), "h.example:8443");
        assert!(parse_https_url("http://example.com/").is_err());
        assert!(parse_https_url("https://user@example.com/").is_err());
        assert!(parse_https_url("https://example.com/a b").is_err());
        assert!(parse_https_url("https://example.com/a\r\nX: y").is_err());
        assert!(parse_https_url("https://example.com:notaport/").is_err());
        assert!(parse_https_url("https:///path").is_err());
    }

    #[test]
    fn resolves_redirects() {
        let from = parse_https_url("https://a.example/x").unwrap();
        assert_eq!(
            resolve_redirect(&from, "/y?z=1").unwrap(),
            "https://a.example/y?z=1"
        );
        assert_eq!(
            resolve_redirect(&from, "https://b.example/").unwrap(),
            "https://b.example/"
        );
        assert!(resolve_redirect(&from, "http://b.example/").is_err());
        assert!(resolve_redirect(&from, "y").is_err());
    }

    #[test]
    fn reads_content_length_bodies() {
        assert_eq!(
            body(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"),
            "hello"
        );
        // Extra bytes after the declared length are not part of the body.
        assert_eq!(
            body(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhixxx"),
            "hi"
        );
        assert!(err(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nhello").contains("closed"));
        assert!(err(b"HTTP/1.1 200 OK\r\nContent-Length: 5000\r\n\r\n").contains("too large"));
        assert!(
            err(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nh")
                .contains("conflicting")
        );
    }

    #[test]
    fn reads_chunked_bodies() {
        let msg = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                    5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\nTrailer: x\r\n\r\n";
        assert_eq!(body(msg), "hello world");
        // Truncated before the final chunk.
        assert!(
            err(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n")
                .contains("closed")
        );
        assert!(
            err(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n")
                .contains("chunk size")
        );
        assert!(
            err(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhelloXX")
                .contains("malformed")
        );
        assert!(
            err(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3e9\r\n")
                .contains("too large")
        );
    }

    #[test]
    fn reads_close_delimited_bodies() {
        assert_eq!(
            body(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nuntil eof"),
            "until eof"
        );
        let mut big = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        big.extend(std::iter::repeat_n(b'x', 1001));
        assert!(err(&big).contains("too large"));
    }

    #[test]
    fn handles_redirects_and_errors() {
        match reply(b"HTTP/1.1 301 Moved\r\nLocation: https://x.example/y\r\n\r\n") {
            Ok(Reply::Redirect(l)) => assert_eq!(l, "https://x.example/y"),
            _ => panic!("expected a redirect"),
        }
        assert!(err(b"HTTP/1.1 302 Found\r\n\r\n").contains("Location"));
        assert!(err(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").contains("404"));
        assert!(err(b"SSH-2.0-OpenSSH\r\n").contains("HTTP/1.x"));
        assert!(err(b"").contains("closed"));
    }

    #[test]
    fn skips_interim_replies_and_rejects_compression() {
        let msg = b"HTTP/1.1 103 Early Hints\r\nLink: </a>\r\n\r\n\
                    HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
        assert_eq!(body(msg), "ok");
        assert!(
            err(b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 1\r\n\r\nx")
                .contains("Content-Encoding")
        );
        assert!(err(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\nx")
            .contains("Transfer-Encoding"));
    }

    #[test]
    fn caps_header_size() {
        let mut msg = b"HTTP/1.1 200 OK\r\n".to_vec();
        for _ in 0..3000 {
            msg.extend_from_slice(b"X-Filler: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
        }
        assert!(err(&msg).contains("too large"));
    }

    /// A stream that records what is written and replays a canned reply.
    struct Mock {
        reply: io::Cursor<Vec<u8>>,
        sent: Vec<u8>,
    }
    impl Read for Mock {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.reply.read(buf)
        }
    }
    impl Write for Mock {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.sent.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn sends_a_plain_request() {
        let mut m = Mock {
            reply: io::Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()),
            sent: Vec::new(),
        };
        let url = parse_https_url("https://h.example:8443/p?q=1").unwrap();
        // `exchange` takes the stream by value; go through a reference.
        let out = exchange(&mut m, &url).unwrap();
        assert!(matches!(out, Reply::Body(b) if b == b"ok"));
        let sent = String::from_utf8(m.sent).unwrap();
        assert!(
            sent.starts_with("GET /p?q=1 HTTP/1.1\r\nHost: h.example:8443\r\n"),
            "{sent}"
        );
        assert!(sent.contains("Accept-Encoding: identity\r\n"));
        assert!(sent.ends_with("\r\n\r\n"));
    }

    #[test]
    fn tunnels_through_a_proxy() {
        let mut m = Mock {
            reply: io::Cursor::new(b"HTTP/1.1 200 Connection established\r\n\r\nTLS".to_vec()),
            sent: Vec::new(),
        };
        let url = parse_https_url("https://h.example/").unwrap();
        let proxy = parse_proxy("http://us%40er:p%3Aw@proxy.local:3128").unwrap();
        tunnel(&mut m, &url, &proxy).unwrap();
        let sent = String::from_utf8(m.sent.clone()).unwrap();
        assert!(sent.starts_with("CONNECT h.example:443 HTTP/1.1\r\nHost: h.example:443\r\n"));
        // base64("us@er:p:w")
        assert!(
            sent.contains("Proxy-Authorization: Basic dXNAZXI6cDp3\r\n"),
            "{sent}"
        );
        // Nothing after the blank line was consumed.
        let mut rest = Vec::new();
        m.reply.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, b"TLS");

        let mut refused = Mock {
            reply: io::Cursor::new(b"HTTP/1.1 403 Forbidden\r\n\r\n".to_vec()),
            sent: Vec::new(),
        };
        assert!(tunnel(&mut refused, &url, &proxy)
            .unwrap_err()
            .contains("403"));
    }

    #[test]
    fn parses_proxy_settings() {
        let p = parse_proxy("http://127.0.0.1:40371").unwrap();
        assert_eq!(
            (p.host.as_str(), p.port, p.auth),
            ("127.0.0.1", 40371, None)
        );
        let p = parse_proxy("proxy.corp").unwrap();
        assert_eq!((p.host.as_str(), p.port), ("proxy.corp", 80));
        assert_eq!(parse_proxy("http://[::1]:8080/").unwrap().host, "::1");
        assert!(parse_proxy("https://proxy:443").is_err());
        assert!(parse_proxy("socks5://proxy:1080").is_err());
        assert!(parse_proxy("http://proxy:99999").is_err());
        // The error must not reveal credentials.
        assert!(!parse_proxy("https://u:secret@p:1")
            .unwrap_err()
            .contains("secret"));
    }

    #[test]
    fn picks_a_proxy_from_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(proxy_for("h.example", &env(&[])).unwrap(), None);
        let e = env(&[
            ("https_proxy", "http://p:1"),
            ("NO_PROXY", "localhost, .internal.example"),
        ]);
        assert!(proxy_for("h.example", &e).unwrap().is_some());
        assert!(proxy_for("db.internal.example", &e).unwrap().is_none());
        assert!(proxy_for("localhost", &e).unwrap().is_none());
        assert!(proxy_for(
            "h.example",
            &env(&[("ALL_PROXY", "http://p:1"), ("NO_PROXY", "*")])
        )
        .unwrap()
        .is_none());
        // Empty values count as unset.
        assert_eq!(proxy_for("h", &env(&[("HTTPS_PROXY", " ")])).unwrap(), None);
    }

    #[test]
    fn matches_no_proxy_lists() {
        let m = no_proxy_matches;
        assert!(m("example.com", "example.com"));
        assert!(m("example.com", "api.example.com"));
        assert!(m(".example.com", "api.example.com"));
        assert!(m("*.example.com", "api.example.com"));
        assert!(!m("example.com", "notexample.com"));
        assert!(!m("10.0.0.0/8, ::1", "example.com"));
        assert!(m("a.com, b.com:8080", "x.b.com"));
        assert!(m("*", "anything.example"));
        assert!(!m("", "example.com"));
    }

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(percent_decode("a%40b%3a"), "a@b:");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
    }
}
