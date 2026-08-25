//! Unix-socket transport for the wire protocol.

use crate::json::{parse, Json};
use crate::proto::{errcode, Frame, Request, Response};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A single frame is capped so that one confused client cannot exhaust the
/// daemon's memory. Generous enough for a plan with a large file listing.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Where the daemon listens. Honours `NOUS_SOCKET`, then falls back to a
/// per-user runtime path, then to the system path.
pub fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("NOUS_SOCKET") {
        return PathBuf::from(p);
    }
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        if !rt.is_empty() {
            return PathBuf::from(rt).join("nous.sock");
        }
    }
    PathBuf::from("/run/nous/nous.sock")
}

/// Root of the daemon's mutable state.
pub fn state_dir() -> PathBuf {
    if let Ok(p) = std::env::var("NOUS_STATE_DIR") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME") {
        if home != "/root" {
            return PathBuf::from(home).join(".local/state/nous");
        }
    }
    PathBuf::from("/var/lib/nous")
}

/// Where policy and configuration are read from, most specific first.
///
/// NOUS installed over an existing distribution lives in the user's own
/// configuration directory and needs no root at all. NOUS as the operating
/// system uses `/etc`. Both are supported at once: the user's directory is
/// consulted first, and site policy in `/etc` still applies underneath it.
pub fn config_dirs() -> Vec<PathBuf> {
    if let Ok(p) = std::env::var("NOUS_CONFIG_DIR") {
        return vec![PathBuf::from(p)];
    }
    let mut dirs = Vec::new();
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            dirs.push(PathBuf::from(xdg).join("nous"));
        }
    } else if let Ok(home) = std::env::var("HOME") {
        if home != "/root" {
            dirs.push(PathBuf::from(home).join(".config/nous"));
        }
    }
    dirs.push(PathBuf::from("/etc/nous"));
    dirs
}

/// The directory configuration is written to, and the first one read.
pub fn config_dir() -> PathBuf {
    config_dirs()
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("/etc/nous"))
}

fn next_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!(
        "{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Read one newline-delimited frame. `Ok(None)` means the peer hung up.
pub fn read_frame<R: BufRead>(r: &mut R) -> Result<Option<Json>, String> {
    let mut line = Vec::new();
    let mut total = 0usize;
    loop {
        let mut byte = [0u8; 1];
        match r.read(&mut byte) {
            Ok(0) => {
                if line.is_empty() {
                    return Ok(None);
                }
                break;
            }
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                total += 1;
                if total > MAX_FRAME_BYTES {
                    return Err(format!("frame exceeds {} bytes", MAX_FRAME_BYTES));
                }
                line.push(byte[0]);
            }
            Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("read error: {}", e)),
        }
    }
    if line.iter().all(|b| b.is_ascii_whitespace()) {
        // Blank keepalive line; ask the caller to read again.
        return read_frame(r);
    }
    let text = String::from_utf8(line).map_err(|_| "frame is not valid UTF-8".to_string())?;
    parse(&text)
        .map(Some)
        .map_err(|e| format!("malformed frame: {}", e))
}

pub fn write_frame<W: Write>(w: &mut W, v: &Json) -> Result<(), String> {
    let mut s = v.to_string();
    s.push('\n');
    w.write_all(s.as_bytes())
        .map_err(|e| format!("write error: {}", e))?;
    w.flush().map_err(|e| format!("flush error: {}", e))
}

/// A connection to the daemon.
pub struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    pub fn connect() -> Result<Client, String> {
        Client::connect_to(&socket_path())
    }

    pub fn connect_to(path: &Path) -> Result<Client, String> {
        let stream = UnixStream::connect(path).map_err(|e| {
            format!(
                "cannot reach nousd at {} ({}). Is the daemon running? Try: nousctl status",
                path.display(),
                e
            )
        })?;
        let reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|e| format!("cannot clone socket: {}", e))?,
        );
        Ok(Client { stream, reader })
    }

    pub fn set_timeout(&self, d: Option<Duration>) -> Result<(), String> {
        self.stream
            .set_read_timeout(d)
            .map_err(|e| format!("cannot set timeout: {}", e))?;
        self.stream
            .set_write_timeout(d)
            .map_err(|e| format!("cannot set timeout: {}", e))
    }

    /// Send a request and wait for the matching response, forwarding any events
    /// that arrive in the meantime to `on_event`.
    pub fn call_with<F>(
        &mut self,
        method: &str,
        params: Json,
        mut on_event: F,
    ) -> Result<Json, String>
    where
        F: FnMut(&crate::proto::Event),
    {
        let id = next_id();
        let req = Request::new(&id, method, params);
        write_frame(&mut self.stream, &req.to_json())?;
        loop {
            let frame = read_frame(&mut self.reader)?
                .ok_or_else(|| "nousd closed the connection".to_string())?;
            match Frame::parse(&frame)? {
                Frame::Evt(e) => on_event(&e),
                Frame::Res(res) if res.id() == id => {
                    return res
                        .into_result()
                        .map_err(|(code, msg)| format!("[{}] {}", code, msg));
                }
                // A response to somebody else's request; ignore it.
                Frame::Res(_) | Frame::Req(_) => continue,
            }
        }
    }

    pub fn call(&mut self, method: &str, params: Json) -> Result<Json, String> {
        self.call_with(method, params, |_| {})
    }

    /// Block reading events, invoking `on_event` until the connection ends.
    pub fn listen<F>(&mut self, mut on_event: F) -> Result<(), String>
    where
        F: FnMut(&crate::proto::Event) -> bool,
    {
        while let Some(frame) = read_frame(&mut self.reader)? {
            if let Ok(Frame::Evt(e)) = Frame::parse(&frame) {
                if !on_event(&e) {
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn send_raw(&mut self, v: &Json) -> Result<(), String> {
        write_frame(&mut self.stream, v)
    }
}

/// Bind the daemon's listening socket, replacing a stale one if the previous
/// daemon died without cleaning up.
pub fn bind(path: &Path) -> Result<UnixListener, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    }
    if path.exists() {
        // If something is still listening, refuse rather than stealing the name.
        if UnixStream::connect(path).is_ok() {
            return Err(format!("nousd is already listening on {}", path.display()));
        }
        std::fs::remove_file(path)
            .map_err(|e| format!("cannot remove stale socket {}: {}", path.display(), e))?;
    }
    let listener =
        UnixListener::bind(path).map_err(|e| format!("cannot bind {}: {}", path.display(), e))?;
    // The socket is the door to every capability on the machine. Owner only.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("cannot secure socket: {}", e))?;
    Ok(listener)
}

/// Turn a transport-level failure into a protocol error response.
pub fn transport_error(id: &str, msg: &str) -> Response {
    Response::err(id, errcode::BAD_REQUEST, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::json_obj;
    use crate::proto::{method, Event};
    use std::io::Cursor;
    use std::thread;

    #[test]
    fn frames_round_trip_through_a_buffer() {
        let mut buf = Vec::new();
        let v = json_obj([("kind", "req".into()), ("id", "1".into())]);
        write_frame(&mut buf, &v).unwrap();
        assert_eq!(*buf.last().unwrap(), b'\n');
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur).unwrap().unwrap(), v);
        assert!(read_frame(&mut cur).unwrap().is_none(), "clean EOF");
    }

    #[test]
    fn blank_lines_are_skipped() {
        let mut cur = Cursor::new(b"\n\n{\"kind\":\"evt\"}\n".to_vec());
        let v = read_frame(&mut cur).unwrap().unwrap();
        assert_eq!(v.str_or("kind", ""), "evt");
    }

    #[test]
    fn malformed_frames_error_rather_than_panic() {
        let mut cur = Cursor::new(b"{not json}\n".to_vec());
        assert!(read_frame(&mut cur).is_err());
    }

    #[test]
    fn oversize_frames_are_rejected() {
        let mut data = vec![b'x'; MAX_FRAME_BYTES + 16];
        data.push(b'\n');
        let mut cur = Cursor::new(data);
        let err = read_frame(&mut cur).unwrap_err();
        assert!(err.contains("exceeds"), "{err}");
    }

    #[test]
    fn client_and_listener_talk_over_a_real_socket() {
        let path = std::env::temp_dir().join(format!("nous-ipc-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = bind(&path).unwrap();

        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let frame = read_frame(&mut reader).unwrap().unwrap();
            let req = Request::from_json(&frame).unwrap();
            // An unsolicited event before the response must not confuse the client.
            let evt = Event::new("log", json_obj([("m", "working".into())]));
            write_frame(&mut writer, &evt.to_json()).unwrap();
            let res = Response::ok(&req.id, json_obj([("pong", true.into())]));
            write_frame(&mut writer, &res.to_json()).unwrap();
        });

        let mut client = Client::connect_to(&path).unwrap();
        let mut seen = 0;
        let out = client
            .call_with(method::PING, Json::obj(), |_| seen += 1)
            .unwrap();
        assert!(out.bool_or("pong", false));
        assert_eq!(seen, 1, "the interleaved event should have been delivered");
        server.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn socket_is_not_world_accessible() {
        let path = std::env::temp_dir().join(format!("nous-perm-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let _l = bind(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "socket must be owner-only, got {:o}", mode);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stale_socket_files_are_reclaimed() {
        let path =
            std::env::temp_dir().join(format!("nous-stale-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"not a socket").unwrap();
        assert!(
            bind(&path).is_ok(),
            "a leftover file must not block startup"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn config_search_prefers_the_user_then_the_system() {
        std::env::remove_var("NOUS_CONFIG_DIR");
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_var("HOME", "/home/joey");
        let dirs = config_dirs();
        assert_eq!(
            dirs.first().unwrap(),
            &PathBuf::from("/home/joey/.config/nous")
        );
        assert_eq!(dirs.last().unwrap(), &PathBuf::from("/etc/nous"));

        // An explicit override replaces the search entirely.
        std::env::set_var("NOUS_CONFIG_DIR", "/opt/nous-config");
        assert_eq!(config_dirs(), vec![PathBuf::from("/opt/nous-config")]);
        std::env::remove_var("NOUS_CONFIG_DIR");
    }

    #[test]
    fn socket_path_honours_the_environment() {
        std::env::set_var("NOUS_SOCKET", "/tmp/explicit.sock");
        assert_eq!(socket_path(), PathBuf::from("/tmp/explicit.sock"));
        std::env::remove_var("NOUS_SOCKET");
    }
}
