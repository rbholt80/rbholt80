//! Minimal leveled logging.
//!
//! Writes to stderr so that journald picks it up when the daemon runs under
//! systemd, and mirrors to a file when one is configured.

use crate::journal::{format_ts, now_secs};
use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl Level {
    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }

    pub fn parse(s: &str) -> Level {
        match s.to_ascii_lowercase().as_str() {
            "debug" | "trace" => Level::Debug,
            "warn" | "warning" => Level::Warn,
            "error" | "err" => Level::Error,
            _ => Level::Info,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
static SINK: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

pub fn set_level(l: Level) {
    LEVEL.store(l as u8, Ordering::Relaxed);
}

pub fn level() -> Level {
    match LEVEL.load(Ordering::Relaxed) {
        0 => Level::Debug,
        2 => Level::Warn,
        3 => Level::Error,
        _ => Level::Info,
    }
}

/// Additionally mirror log lines to `path`.
pub fn set_file(path: &std::path::Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create log dir: {}", e))?;
    }
    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("cannot open log file {}: {}", path.display(), e))?;
    let cell = SINK.get_or_init(|| Mutex::new(None));
    *cell.lock().unwrap() = Some(f);
    Ok(())
}

pub fn log(l: Level, module: &str, msg: &str) {
    if l < level() {
        return;
    }
    let line = format!(
        "{} {:<5} [{}] {}",
        format_ts(now_secs()),
        l.as_str(),
        module,
        msg
    );
    eprintln!("{}", line);
    if let Some(cell) = SINK.get() {
        if let Ok(mut guard) = cell.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = writeln!(f, "{}", line);
            }
        }
    }
}

#[macro_export]
macro_rules! log_debug {
    ($m:expr, $($arg:tt)*) => { $crate::log::log($crate::log::Level::Debug, $m, &format!($($arg)*)) };
}
#[macro_export]
macro_rules! log_info {
    ($m:expr, $($arg:tt)*) => { $crate::log::log($crate::log::Level::Info, $m, &format!($($arg)*)) };
}
#[macro_export]
macro_rules! log_warn {
    ($m:expr, $($arg:tt)*) => { $crate::log::log($crate::log::Level::Warn, $m, &format!($($arg)*)) };
}
#[macro_export]
macro_rules! log_error {
    ($m:expr, $($arg:tt)*) => { $crate::log::log($crate::log::Level::Error, $m, &format!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_level_names() {
        assert_eq!(Level::parse("debug"), Level::Debug);
        assert_eq!(Level::parse("WARN"), Level::Warn);
        assert_eq!(Level::parse("nonsense"), Level::Info);
    }

    #[test]
    fn levels_order_by_severity() {
        assert!(Level::Debug < Level::Info);
        assert!(Level::Error > Level::Warn);
    }
}
