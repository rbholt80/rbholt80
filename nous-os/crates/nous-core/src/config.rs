//! Configuration.
//!
//! An INI-ish `key = value` file with `[section]` headers. Unknown keys are
//! preserved and readable, so an agent can carry its own settings in the same
//! file without the core needing to know about them.

use crate::ipc::state_dir;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    values: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Config::with_defaults()
    }
}

impl Config {
    pub fn with_defaults() -> Config {
        let mut values = BTreeMap::new();
        let mut set = |k: &str, v: &str| {
            values.insert(k.to_string(), v.to_string());
        };

        // Model routing. Backends are tried in order; the first that is
        // reachable wins. `offline` is last but always succeeds, which is what
        // keeps the shell usable on a machine with no model at all.
        // Two routes, because most of what an AI-native OS does all day is
        // small: naming a file, classifying a download, summarising a folder.
        // That work goes to a local model and never leaves the machine. The
        // large route is only reached when an intent genuinely needs it.
        set("model.route", "ollama,anthropic,openai,offline");
        set("model.route.small", "ollama,offline");
        set("model.route.large", "anthropic,openai,ollama,offline");
        set("model.timeout_secs", "60");
        set("model.max_context_chars", "24000");

        set("model.ollama.url", "http://127.0.0.1:11434");
        set("model.ollama.model", "qwen2.5:7b-instruct");
        // The bundled small model: a few hundred megabytes, good enough for
        // classification and naming, and it runs on a laptop with no GPU.
        set("model.ollama.small_model", "qwen2.5:1.5b-instruct");

        set(
            "model.anthropic.url",
            "https://api.anthropic.com/v1/messages",
        );
        set("model.anthropic.model", "claude-sonnet-5");
        set("model.anthropic.max_tokens", "2048");
        // Read from the environment by default so the key is never in a file.
        set("model.anthropic.key_env", "ANTHROPIC_API_KEY");

        // Any OpenAI-compatible endpoint: OpenAI itself, OpenRouter, Groq, a
        // local llama.cpp or LM Studio server. Bring whichever key you hold.
        set(
            "model.openai.url",
            "https://api.openai.com/v1/chat/completions",
        );
        set("model.openai.model", "gpt-4o-mini");
        set("model.openai.provider", "openai");

        // The resolver prefers the grammar when it is confident; below this it
        // escalates to a model, and below `plan.show_threshold` it shows you the
        // plan before doing anything.
        // Named assistants, reachable by typing their name first: "claude what
        // is this error", "chatgpt rewrite this". Add your own by adding a
        // section -- any OpenAI-compatible endpoint works.
        set("assist.claude.backend", "anthropic");
        set("assist.claude.model", "claude-sonnet-5");
        set("assist.claude.aliases", "claude,cl");

        set("assist.chatgpt.backend", "openai");
        set("assist.chatgpt.model", "gpt-4o");
        set("assist.chatgpt.aliases", "chatgpt,gpt,openai");

        set("assist.local.backend", "ollama");
        set("assist.local.model", "");
        set("assist.local.aliases", "local,ollama,llama");

        set("plan.grammar_threshold", "0.72");
        set("plan.show_threshold", "0.55");
        set("plan.max_steps", "12");

        // What the semantic index is allowed to look at.
        set("index.roots", "~/Documents,~/Downloads,~/Projects,~/notes");
        set("index.max_file_bytes", "1048576");
        set(
            "index.exclude",
            "node_modules,.git,target,__pycache__,.venv,dist,build",
        );
        set("index.interval_secs", "900");

        // What NOUS keeps on your behalf, and for how long. A system that
        // warns you about disk space must not be the reason you run out.
        set("retain.journal_records", "20000");
        set("retain.journal_archives", "4");
        set("retain.backup_mb", "2048");
        set("retain.trash_days", "30");
        set("retain.thumbnail_days", "60");
        set("retain.screenshot_days", "14");
        set("retain.interval_secs", "21600");

        set("sensor.interval_secs", "20");
        set("sensor.load_alert", "4.0");
        set("sensor.disk_alert_pct", "92");
        set("sensor.mem_alert_pct", "90");

        set("agent.dir", "/usr/lib/nous/agents:/etc/nous/agents");
        set("agent.start_timeout_secs", "10");

        set("log.level", "info");

        Config { values }
    }

    /// Load `nous.conf` from every configuration directory, layered over the
    /// defaults. A missing file is not an error — the defaults are a working
    /// system. Directories are read least-specific first so that the user's own
    /// configuration wins over the system's.
    pub fn load() -> Config {
        let mut cfg = Config::with_defaults();
        for dir in crate::ipc::config_dirs().into_iter().rev() {
            let path = dir.join("nous.conf");
            if let Ok(text) = std::fs::read_to_string(&path) {
                match Config::parse(&text) {
                    Ok(file) => cfg.merge(file),
                    Err(e) => eprintln!("nous: ignoring {}: {}", path.display(), e),
                }
            }
        }
        cfg
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let mut values = BTreeMap::new();
        let mut section = String::new();
        for (i, raw) in text.lines().enumerate() {
            let line = match raw.find('#') {
                Some(p) => &raw[..p],
                None => raw,
            }
            .trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                section = name.trim().to_string();
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected 'key = value'", i + 1))?;
            let key = if section.is_empty() {
                k.trim().to_string()
            } else {
                format!("{}.{}", section, k.trim())
            };
            values.insert(key, v.trim().trim_matches('"').to_string());
        }
        Ok(Config { values })
    }

    pub fn merge(&mut self, other: Config) {
        self.values.extend(other.values);
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }

    pub fn str_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).unwrap_or(default)
    }

    pub fn f64_or(&self, key: &str, default: f64) -> f64 {
        self.get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    pub fn u64_or(&self, key: &str, default: u64) -> u64 {
        self.get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    pub fn bool_or(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            Some(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
            None => default,
        }
    }

    /// A comma-separated list, trimmed, with empties dropped.
    pub fn list(&self, key: &str) -> Vec<String> {
        self.get(key)
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A list of paths with `~` expanded.
    pub fn paths(&self, key: &str) -> Vec<PathBuf> {
        self.list(key).iter().map(|s| expand_tilde(s)).collect()
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.values.insert(key.to_string(), value.to_string());
    }

    pub fn keys_under(&self, prefix: &str) -> Vec<(&str, &str)> {
        self.values
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    // Common derived paths.
    pub fn journal_dir(&self) -> PathBuf {
        state_dir().join("journal")
    }
    pub fn index_dir(&self) -> PathBuf {
        state_dir().join("index")
    }
    pub fn memory_dir(&self) -> PathBuf {
        state_dir().join("memory")
    }
}

pub fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    if s == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_a_working_system() {
        let c = Config::with_defaults();
        assert!(c.list("model.route").contains(&"offline".to_string()));
        assert!(c.u64_or("model.timeout_secs", 0) > 0);
    }

    #[test]
    fn the_named_assistants_are_configured() {
        let c = Config::with_defaults();
        assert_eq!(c.get("assist.claude.backend"), Some("anthropic"));
        assert!(c
            .list("assist.chatgpt.aliases")
            .contains(&"gpt".to_string()));
        // The local one is named too, so "keep this on my machine" is one word.
        assert_eq!(c.get("assist.local.backend"), Some("ollama"));
    }

    #[test]
    fn everything_kept_has_a_bound() {
        // Every store NOUS writes to needs a limit, or it is an unbounded copy
        // of the user's disk.
        let c = Config::with_defaults();
        for key in [
            "retain.journal_records",
            "retain.journal_archives",
            "retain.backup_mb",
            "retain.trash_days",
            "retain.thumbnail_days",
            "retain.screenshot_days",
        ] {
            assert!(c.u64_or(key, 0) > 0, "{} has no bound", key);
        }
    }

    #[test]
    fn sections_become_dotted_keys() {
        let c = Config::parse("[model]\nroute = offline\n\n[index]\nroots = ~/a,~/b\n").unwrap();
        assert_eq!(c.get("model.route"), Some("offline"));
        assert_eq!(c.list("index.roots"), vec!["~/a", "~/b"]);
    }

    #[test]
    fn comments_and_quotes_are_stripped() {
        let c = Config::parse("key = \"a value\"   # trailing note\n").unwrap();
        assert_eq!(c.get("key"), Some("a value"));
    }

    #[test]
    fn file_values_override_defaults_only_where_present() {
        let mut c = Config::with_defaults();
        let before = c.str_or("model.ollama.model", "").to_string();
        c.merge(Config::parse("[model]\nroute = offline\n").unwrap());
        assert_eq!(c.get("model.route"), Some("offline"));
        assert_eq!(
            c.str_or("model.ollama.model", ""),
            before,
            "untouched keys must survive"
        );
    }

    #[test]
    fn malformed_lines_are_reported() {
        assert!(Config::parse("this line has no equals sign\n").is_err());
    }

    #[test]
    fn tilde_expands_against_home() {
        // Reads HOME rather than setting it: tests share one process.
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            assert_eq!(
                expand_tilde("~/Documents"),
                PathBuf::from(&home).join("Documents")
            );
        }
        assert_eq!(expand_tilde("/etc/nous"), PathBuf::from("/etc/nous"));
    }

    #[test]
    fn booleans_accept_the_usual_spellings() {
        let c = Config::parse("a = yes\nb = off\nc = 1\n").unwrap();
        assert!(c.bool_or("a", false));
        assert!(!c.bool_or("b", true));
        assert!(c.bool_or("c", false));
        assert!(
            c.bool_or("missing", true),
            "absent keys fall back to the default"
        );
    }
}
