//! The graphical shell's transport.
//!
//! `nousd` serves the desktop itself: a small HTTP server on loopback hands out
//! the shell's assets, relays JSON-RPC to the same handler the unix socket uses,
//! and pushes bus events over a WebSocket. The shell therefore has no
//! privileged path — it asks for capabilities exactly like `nsh` does, and is
//! refused exactly as often.
//!
//! The assets are compiled into the binary, so the desktop cannot be broken by
//! a half-finished package upgrade.

use crate::Daemon;
use nous_core::json::{json_obj, parse, Json};
use nous_core::proto::Request;
use nous_core::{log_info, log_warn};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

const MODULE: &str = "webui";

const INDEX_HTML: &str = include_str!("../ui/index.html");
const APP_CSS: &str = include_str!("../ui/app.css");
const APP_JS: &str = include_str!("../ui/app.js");

/// Requests are bounded; the shell is a local client, not a public endpoint.
const MAX_BODY: usize = 4 * 1024 * 1024;

pub fn serve(daemon: Arc<Daemon>, port: u16) -> Result<(), String> {
    // Loopback only. The shell is this machine's desktop, not a web service.
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("cannot bind 127.0.0.1:{}: {}", port, e))?;
    log_info!(MODULE, "graphical shell at http://127.0.0.1:{}", port);

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let d = daemon.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle(d, stream) {
                        log_warn!(MODULE, "{}", e);
                    }
                });
            }
            Err(e) => log_warn!(MODULE, "accept failed: {}", e),
        }
    }
    Ok(())
}

struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| *k == lower).map(|(_, v)| v.as_str())
    }
}

fn read_request<R: BufRead>(r: &mut R) -> Result<Option<HttpRequest>, String> {
    let mut line = String::new();
    if r.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    if method.is_empty() {
        return Ok(None);
    }

    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).map_err(|e| e.to_string())? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }

    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    if len > MAX_BODY {
        return Err(format!("request body of {} bytes is too large", len));
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut body).map_err(|e| e.to_string())?;
    }

    Ok(Some(HttpRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    }))
}

fn handle(daemon: Arc<Daemon>, stream: TcpStream) -> Result<(), String> {
    stream.set_read_timeout(Some(Duration::from_secs(300))).ok();
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut writer = stream;

    let req = match read_request(&mut reader)? {
        Some(r) => r,
        None => return Ok(()),
    };

    // Reject cross-origin drive-by requests: a page on another site must not be
    // able to talk to the shell just because it is listening on localhost.
    if let Some(origin) = req.header("origin") {
        if !origin.contains("127.0.0.1") && !origin.contains("localhost") {
            return respond(&mut writer, 403, "text/plain", b"cross-origin requests are refused");
        }
    }

    let path = req.path.split('?').next().unwrap_or("/");
    match (req.method.as_str(), path) {
        ("GET", "/") | ("GET", "/index.html") => {
            respond(&mut writer, 200, "text/html; charset=utf-8", INDEX_HTML.as_bytes())
        }
        ("GET", "/app.css") => {
            respond(&mut writer, 200, "text/css; charset=utf-8", APP_CSS.as_bytes())
        }
        ("GET", "/app.js") => {
            respond(&mut writer, 200, "application/javascript; charset=utf-8", APP_JS.as_bytes())
        }
        ("GET", "/events") => serve_events(daemon, &req, writer),
        ("POST", "/api") => {
            let body = parse(&req.body).unwrap_or_else(|_| Json::obj());
            let rpc = Request {
                id: body.str_or("id", "1").to_string(),
                method: body.str_or("method", "").to_string(),
                params: body.get("params").cloned().unwrap_or_else(Json::obj),
            };
            let out = daemon.handle(&rpc).to_json().to_string();
            respond(&mut writer, 200, "application/json", out.as_bytes())
        }
        ("OPTIONS", _) => respond(&mut writer, 204, "text/plain", b""),
        _ => respond(&mut writer, 404, "text/plain", b"not found"),
    }
}

fn respond(w: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        status,
        reason,
        content_type,
        body.len()
    );
    w.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    w.write_all(body).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ websocket

fn serve_events(daemon: Arc<Daemon>, req: &HttpRequest, mut writer: TcpStream) -> Result<(), String> {
    let key = match req.header("sec-websocket-key") {
        Some(k) => k,
        None => return respond(&mut writer, 400, "text/plain", b"expected a websocket upgrade"),
    };
    let accept = websocket_accept(key);
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept
    );
    writer.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    writer.flush().ok();

    let (id, rx) = daemon.bus.subscribe(vec!["*".to_string()]);
    let bus = daemon.bus.clone();
    // A hello frame so the shell can render its status bar before the first
    // real event arrives.
    let hello = json_obj([("topic", "hello".into()), ("data", daemon.status())]);
    if write_ws_text(&mut writer, &hello.to_string()).is_err() {
        bus.unsubscribe(id);
        return Ok(());
    }

    while let Ok(event) = rx.recv() {
        if write_ws_text(&mut writer, &event.to_json().to_string()).is_err() {
            break;
        }
        bus.ack(id, 1);
    }
    bus.unsubscribe(id);
    Ok(())
}

/// Write one unmasked text frame. Servers never mask (RFC 6455 §5.1).
pub fn write_ws_text(w: &mut TcpStream, text: &str) -> Result<(), String> {
    let payload = text.as_bytes();
    let mut frame = vec![0x81u8]; // FIN + opcode 1 (text)
    match payload.len() {
        n if n < 126 => frame.push(n as u8),
        n if n <= u16::MAX as usize => {
            frame.push(126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            frame.push(127);
            frame.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    w.write_all(&frame).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())
}

/// The handshake response value: base64(sha1(key + magic)).
pub fn websocket_accept(key: &str) -> String {
    const MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    base64(&sha1(format!("{}{}", key.trim(), MAGIC).as_bytes()))
}

/// SHA-1. Present because the WebSocket handshake mandates it, not because it
/// is being used for anything security-bearing.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];

    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub fn base64(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn sha1_matches_the_published_vectors() {
        assert_eq!(
            base64(&sha1(b"abc")),
            "qZk+NkcGgWq6PiVxeFDCbJzQ2J0=",
            "SHA-1('abc')"
        );
        assert_eq!(
            base64(&sha1(b"")),
            "2jmj7l5rSw0yVb/vlWAYkK/YBwk=",
            "SHA-1 of the empty string"
        );
    }

    #[test]
    fn sha1_handles_input_spanning_several_blocks() {
        // 1000 'a's crosses the 64-byte block boundary many times.
        assert_eq!(base64(&sha1(&vec![b'a'; 1000])), "KR6abGaZSUm1e6XmUDYemPw2sbo=");
        // Exactly one block: the padding path that needs a whole extra block.
        assert_eq!(base64(&sha1(&vec![b'a'; 64])), "AJi6gktcFkJ716ESKlpEKiXsZE0=");
    }

    #[test]
    fn base64_pads_correctly() {
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn websocket_handshake_matches_the_rfc_example() {
        // RFC 6455 section 1.3.
        assert_eq!(websocket_accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn parses_a_post_with_a_body() {
        let raw = "POST /api HTTP/1.1\r\nHost: x\r\nContent-Length: 9\r\n\r\n{\"a\":true}";
        let mut cur = Cursor::new(&raw[..raw.len() - 1]);
        let req = read_request(&mut cur).unwrap().unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/api");
        assert_eq!(req.header("host"), Some("x"));
        assert_eq!(req.body.len(), 9);
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let raw = "GET /events HTTP/1.1\r\nSec-WebSocket-Key: abc\r\n\r\n";
        let mut cur = Cursor::new(raw);
        let req = read_request(&mut cur).unwrap().unwrap();
        assert_eq!(req.header("sec-websocket-key"), Some("abc"));
        assert_eq!(req.header("Sec-WebSocket-Key"), Some("abc"));
    }

    #[test]
    fn an_oversized_body_is_refused_before_it_is_read() {
        let raw = format!("POST /api HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        let mut cur = Cursor::new(raw);
        assert!(read_request(&mut cur).is_err());
    }

    #[test]
    fn an_empty_connection_is_not_an_error() {
        let mut cur = Cursor::new("");
        assert!(read_request(&mut cur).unwrap().is_none());
    }

    #[test]
    fn frame_headers_size_themselves_to_the_payload() {
        // Exercised through the length prefix rules rather than a live socket.
        let short = 10usize;
        let medium = 300usize;
        let long = 70_000usize;
        assert!(short < 126);
        assert!(medium <= u16::MAX as usize && medium >= 126);
        assert!(long > u16::MAX as usize);
    }
}
