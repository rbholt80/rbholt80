//! A small HTTP client.
//!
//! Plain HTTP is spoken directly over TCP — that covers a local model server,
//! which is the case that has to work without anything installed. HTTPS is
//! delegated to `curl`, because shipping a TLS stack is not something a
//! dependency-free core can honestly do, and pretending otherwise would mean
//! writing certificate validation by hand.

use nous_core::json::{parse, Json};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    pub fn json(&self) -> Result<Json, String> {
        parse(&self.body).map_err(|e| format!("response was not JSON: {} (body: {})", e, truncate(&self.body, 200)))
    }

    pub fn require_ok(&self) -> Result<&Self, String> {
        if (200..300).contains(&self.status) {
            Ok(self)
        } else {
            Err(format!("HTTP {}: {}", self.status, truncate(&self.body, 400)))
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

/// Split a URL into (scheme, host, port, path).
pub fn split_url(url: &str) -> Result<(String, String, u16, String), String> {
    let (scheme, rest) = url.split_once("://").ok_or_else(|| format!("malformed URL '{}'", url))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().map_err(|_| format!("bad port in '{}'", url))?),
        None => (authority.to_string(), if scheme == "https" { 443 } else { 80 }),
    };
    Ok((scheme.to_string(), host, port, path.to_string()))
}

/// POST a JSON body. Routes to TCP or curl based on the scheme.
pub fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &Json,
    timeout: Duration,
) -> Result<HttpResponse, String> {
    let (scheme, host, port, path) = split_url(url)?;
    let payload = body.to_string();
    if scheme == "https" {
        return post_via_curl(url, headers, &payload, timeout);
    }
    post_plain(&host, port, &path, headers, &payload, timeout)
}

fn post_plain(
    host: &str,
    port: u16,
    path: &str,
    headers: &[(&str, &str)],
    payload: &str,
    timeout: Duration,
) -> Result<HttpResponse, String> {
    let addr = format!("{}:{}", host, port);
    let mut stream = TcpStream::connect(&addr)
        .map_err(|e| format!("cannot connect to {}: {}", addr, e))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();

    let mut req = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        path,
        host,
        payload.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    req.push_str("\r\n");
    req.push_str(payload);

    stream.write_all(req.as_bytes()).map_err(|e| format!("write failed: {}", e))?;
    stream.flush().ok();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| format!("read failed: {}", e))?;
    let text = String::from_utf8_lossy(&raw).to_string();
    parse_response(&text)
}

/// Parse a raw HTTP/1.1 response, including chunked transfer encoding.
pub fn parse_response(text: &str) -> Result<HttpResponse, String> {
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed HTTP response (no header terminator)".to_string())?;
    let status_line = head.lines().next().unwrap_or("");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("malformed status line '{}'", status_line))?;

    let chunked = head
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.to_ascii_lowercase().contains("chunked"));

    let body = if chunked { dechunk(body)? } else { body.to_string() };
    Ok(HttpResponse { status, body })
}

fn dechunk(body: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = body;
    loop {
        let (size_line, remainder) = match rest.split_once("\r\n") {
            Some(x) => x,
            None => break,
        };
        let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0").trim(), 16)
            .map_err(|_| format!("bad chunk size '{}'", size_line.trim()))?;
        if size == 0 {
            break;
        }
        if remainder.len() < size {
            return Err("truncated chunk".to_string());
        }
        out.push_str(&remainder[..size]);
        rest = remainder[size..].trim_start_matches("\r\n");
    }
    Ok(out)
}

fn post_via_curl(
    url: &str,
    headers: &[(&str, &str)],
    payload: &str,
    timeout: Duration,
) -> Result<HttpResponse, String> {
    if !crate::exec::sysops::have("curl") {
        return Err("curl is required for HTTPS model backends but is not installed".to_string());
    }
    let mut args: Vec<String> = vec![
        "-s".into(),
        "-S".into(),
        // Write the status code after the body so both come back in one stream.
        "-w".into(),
        "\n%{http_code}".into(),
        "--max-time".into(),
        timeout.as_secs().to_string(),
        "-X".into(),
        "POST".into(),
        "-H".into(),
        "Content-Type: application/json".into(),
    ];
    for (k, v) in headers {
        args.push("-H".into());
        args.push(format!("{}: {}", k, v));
    }
    // The payload goes in on stdin so secrets never appear in the process list.
    args.push("--data-binary".into());
    args.push("@-".into());
    args.push(url.to_string());

    use std::process::{Command, Stdio};
    let mut child = Command::new("curl")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run curl: {}", e))?;
    child
        .stdin
        .as_mut()
        .ok_or("cannot write to curl")?
        .write_all(payload.as_bytes())
        .map_err(|e| format!("cannot send request body: {}", e))?;
    let out = child.wait_with_output().map_err(|e| format!("curl failed: {}", e))?;
    if !out.status.success() {
        return Err(format!("curl failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, code) = text.rsplit_once('\n').unwrap_or((text.as_str(), "0"));
    Ok(HttpResponse { status: code.trim().parse().unwrap_or(0), body: body.to_string() })
}

/// Is something listening at this URL's host and port?
pub fn reachable(url: &str, timeout: Duration) -> bool {
    let (_, host, port, _) = match split_url(url) {
        Ok(x) => x,
        Err(_) => return false,
    };
    use std::net::ToSocketAddrs;
    let addr = match format!("{}:{}", host, port).to_socket_addrs() {
        Ok(mut a) => match a.next() {
            Some(a) => a,
            None => return false,
        },
        Err(_) => return false,
    };
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_urls_into_parts() {
        assert_eq!(
            split_url("http://127.0.0.1:11434/api/generate").unwrap(),
            ("http".into(), "127.0.0.1".into(), 11434, "/api/generate".into())
        );
        assert_eq!(
            split_url("https://api.anthropic.com/v1/messages").unwrap(),
            ("https".into(), "api.anthropic.com".into(), 443, "/v1/messages".into())
        );
        // No path means root.
        assert_eq!(split_url("http://localhost").unwrap().3, "/");
        assert!(split_url("not-a-url").is_err());
    }

    #[test]
    fn parses_a_plain_response() {
        let raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"response\":\"hi\"}";
        let r = parse_response(raw).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.json().unwrap().str_or("response", ""), "hi");
        assert!(r.require_ok().is_ok());
    }

    #[test]
    fn parses_a_chunked_response() {
        let raw = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"a\":\r\n2\r\n1}\r\n0\r\n\r\n";
        let r = parse_response(raw).unwrap();
        assert_eq!(r.body, "{\"a\":1}");
        assert_eq!(r.json().unwrap().f64_or("a", 0.0), 1.0);
    }

    #[test]
    fn non_2xx_is_an_error_with_the_body_attached() {
        let raw = "HTTP/1.1 401 Unauthorized\r\n\r\n{\"error\":\"bad key\"}";
        let r = parse_response(raw).unwrap();
        let err = r.require_ok().unwrap_err();
        assert!(err.contains("401"), "{err}");
        assert!(err.contains("bad key"), "{err}");
    }

    #[test]
    fn malformed_responses_error_rather_than_panic() {
        assert!(parse_response("garbage").is_err());
        assert!(parse_response("HTTP/1.1\r\n\r\nbody").is_err());
    }

    #[test]
    fn non_json_bodies_report_themselves_readably() {
        let r = HttpResponse { status: 200, body: "<html>gateway error</html>".into() };
        let err = r.json().unwrap_err();
        assert!(err.contains("was not JSON"), "{err}");
    }

    #[test]
    fn nothing_is_listening_on_an_unused_port() {
        assert!(!reachable("http://127.0.0.1:1", Duration::from_millis(200)));
    }
}
