//! Credential storage for model providers.
//!
//! This lives in the core rather than the daemon because `nousctl` must be able
//! to store a key on a machine where the daemon will not start -- which is
//! exactly when you most need to configure one. Two implementations of the same
//! file format would drift, and the one that drifted would be the one holding
//! your credentials.
//!
//! API keys live in a single owner-only file that is on the capability system's
//! protected-read list, which means no capability — and therefore no agent, no
//! flow, and no model — can read it back. Keys go *out* to the provider the
//! user configured and nowhere else.

use crate::json::{json_obj, Json};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

pub fn secrets_path() -> PathBuf {
    if let Ok(p) = std::env::var("NOUS_SECRETS") {
        return PathBuf::from(p);
    }
    if let Ok(home) = std::env::var("HOME") {
        if home != "/root" {
            return PathBuf::from(home).join(".config/nous/secrets/providers.conf");
        }
    }
    PathBuf::from("/etc/nous/secrets/providers.conf")
}

#[derive(Debug, Clone, Default)]
pub struct Secrets {
    keys: BTreeMap<String, String>,
}

impl Secrets {
    /// Load from disk, then overlay the environment.
    ///
    /// The environment wins so a user can run a one-off session with a
    /// different key without editing anything.
    pub fn load() -> Secrets {
        Secrets::load_from(&secrets_path())
    }

    pub fn load_from(path: &std::path::Path) -> Secrets {
        let mut s = Secrets::default();
        if let Ok(text) = std::fs::read_to_string(path) {
            s.merge_text(&text);
        }
        for (provider, var) in [
            ("anthropic", "ANTHROPIC_API_KEY"),
            ("openai", "OPENAI_API_KEY"),
            ("openrouter", "OPENROUTER_API_KEY"),
        ] {
            if let Ok(v) = std::env::var(var) {
                if !v.trim().is_empty() {
                    s.keys.insert(provider.to_string(), v.trim().to_string());
                }
            }
        }
        s
    }

    fn merge_text(&mut self, text: &str) {
        for line in text.lines() {
            let line = match line.find('#') {
                Some(i) => &line[..i],
                None => line,
            }
            .trim();
            if line.is_empty() {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"');
                if !v.is_empty() {
                    self.keys
                        .insert(k.trim().to_ascii_lowercase(), v.to_string());
                }
            }
        }
    }

    pub fn get(&self, provider: &str) -> Option<&str> {
        self.keys
            .get(provider)
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
    }

    pub fn has(&self, provider: &str) -> bool {
        self.get(provider).is_some()
    }

    pub fn providers(&self) -> Vec<&str> {
        self.keys.keys().map(|s| s.as_str()).collect()
    }

    /// Store a key, creating the file owner-only.
    pub fn set(provider: &str, key: &str) -> Result<(), String> {
        Secrets::set_at(&secrets_path(), provider, key)
    }

    pub fn set_at(path: &std::path::Path, provider: &str, key: &str) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).ok();
        }
        let mut existing = Secrets::default();
        if let Ok(text) = std::fs::read_to_string(path) {
            existing.merge_text(&text);
        }
        existing
            .keys
            .insert(provider.to_ascii_lowercase(), key.trim().to_string());

        let mut body = String::from("# NOUS model provider credentials.\n# This file is owner-only and unreadable through any capability.\n");
        for (k, v) in &existing.keys {
            body.push_str(&format!("{} = {}\n", k, v));
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
        f.write_all(body.as_bytes())
            .map_err(|e| format!("cannot write credentials: {}", e))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
        Ok(())
    }

    /// A description safe to show in a UI: which providers are configured, and
    /// never any key material.
    pub fn status(&self) -> Json {
        let list: Vec<Json> = ["anthropic", "openai", "openrouter", "ollama"]
            .iter()
            .map(|p| {
                json_obj([
                    ("provider", (*p).into()),
                    // ollama is local and needs no key at all.
                    ("configured", (self.has(p) || *p == "ollama").into()),
                    ("needs_key", (*p != "ollama").into()),
                ])
            })
            .collect();
        json_obj([
            ("providers", Json::Arr(list)),
            ("path", secrets_path().to_string_lossy().to_string().into()),
        ])
    }
}

/// Redact anything that looks like a key before it reaches a log or an event.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_inclusive(char::is_whitespace) {
        let t = word.trim();
        let looks_like_a_key = t.len() > 20
            && (t.starts_with("sk-") || t.starts_with("sk_") || t.starts_with("key-"))
            || (t.len() > 32
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && t.chars().any(|c| c.is_ascii_digit())
                && t.chars().any(|c| c.is_ascii_uppercase()));
        if looks_like_a_key {
            out.push_str("[redacted]");
            out.push_str(&word[t.len()..]);
        } else {
            out.push_str(word);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private credential file per test. Explicit paths rather than an
    /// environment variable, so tests running in parallel cannot clobber
    /// each other's configuration.
    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join(format!("nous-secrets-{}-{}", tag, std::process::id()))
            .join("providers.conf");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
        p
    }

    #[test]
    fn stores_keys_owner_only() {
        let path = scratch("perms");
        Secrets::set_at(&path, "anthropic", "sk-ant-example").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "credentials must not be group or world readable"
        );
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "the containing directory must be private too"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn round_trips_several_providers() {
        let path = scratch("roundtrip");
        Secrets::set_at(&path, "anthropic", "sk-ant-1").unwrap();
        Secrets::set_at(&path, "openai", "sk-openai-2").unwrap();
        let s = Secrets::load_from(&path);
        assert_eq!(s.get("anthropic"), Some("sk-ant-1"));
        assert_eq!(
            s.get("openai"),
            Some("sk-openai-2"),
            "storing a second key must not drop the first"
        );
        assert!(!s.has("openrouter"));
        assert_eq!(s.providers().len(), 2);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn status_never_leaks_key_material() {
        let path = scratch("status");
        Secrets::set_at(&path, "anthropic", "sk-ant-super-secret-value").unwrap();
        let rendered = Secrets::load_from(&path).status().to_string();
        assert!(!rendered.contains("super-secret"), "{rendered}");
        assert!(rendered.contains("anthropic"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn ollama_counts_as_configured_without_a_key() {
        let s = Secrets::default();
        let providers = s.status().arr_or_empty("providers");
        let ollama = providers
            .iter()
            .find(|p| p.str_or("provider", "") == "ollama")
            .unwrap();
        assert!(ollama.bool_or("configured", false));
        assert!(!ollama.bool_or("needs_key", true));
        let anthropic = providers
            .iter()
            .find(|p| p.str_or("provider", "") == "anthropic")
            .unwrap();
        assert!(
            !anthropic.bool_or("configured", true),
            "no key means not configured"
        );
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let mut s = Secrets::default();
        s.merge_text("# a note\n\nanthropic = sk-1  # trailing\nempty =\n");
        assert_eq!(s.get("anthropic"), Some("sk-1"));
        assert!(
            s.get("empty").is_none(),
            "an empty value is not a credential"
        );
    }

    #[test]
    fn the_secrets_path_is_on_the_protected_read_list() {
        // The whole design rests on this: no capability can read the file back.
        let path = "/home/someone/.config/nous/secrets/providers.conf";
        let cap = crate::Capability::parse(&format!("fs.read:{}", path)).unwrap();
        assert!(
            crate::cap::protected_violation(&cap).is_some(),
            "credentials must be unreadable through the capability system"
        );
    }

    #[test]
    fn redaction_hides_keys_but_keeps_prose() {
        let s = redact("using sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAA now");
        assert!(!s.contains("api03"), "{s}");
        assert!(s.contains("using"), "{s}");
        assert!(s.contains("now"), "{s}");
        assert_eq!(
            redact("a short note about files"),
            "a short note about files"
        );
    }
}
