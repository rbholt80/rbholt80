//! Named assistants.
//!
//! Type `claude what is this error` or `chatgpt rewrite this paragraph` into
//! the same box you use for everything else. No window to find, no application
//! to switch to, no tab that was already open somewhere.
//!
//! The registry is configuration, not code, so adding one is three lines in a
//! file. Anything speaking the OpenAI shape works, which is most of them.
//!
//! One decision worth stating plainly: agents are denied `assist.ask` outright.
//! An agent that could "ask an assistant" could put anything it had read into
//! the question, and that is an exfiltration channel wearing a friendly hat.

use crate::router::{Completion, Router, Tier};
use nous_core::json::{json_obj, Json};
use nous_core::Config;

#[derive(Debug, Clone)]
pub struct Assistant {
    /// The canonical name, e.g. `claude`.
    pub name: String,
    /// Which model backend serves it.
    pub backend: String,
    /// Model override, or empty to use the backend's configured default.
    pub model: String,
    /// Everything you can type to reach it.
    pub aliases: Vec<String>,
}

impl Assistant {
    /// Does what the user typed reach this assistant?
    pub fn answers_to(&self, word: &str) -> bool {
        let w = word
            .trim()
            .trim_end_matches([',', ':'])
            .to_ascii_lowercase();
        w == self.name || self.aliases.iter().any(|a| a.to_ascii_lowercase() == w)
    }

    /// Does asking this one send your words off the machine?
    pub fn is_local(&self) -> bool {
        self.backend == "ollama"
    }

    pub fn to_json(&self, available: bool) -> Json {
        json_obj([
            ("name", self.name.clone().into()),
            ("backend", self.backend.clone().into()),
            ("model", self.model.clone().into()),
            (
                "aliases",
                Json::Arr(self.aliases.iter().map(|a| Json::Str(a.clone())).collect()),
            ),
            ("local", self.is_local().into()),
            ("available", available.into()),
        ])
    }
}

/// Read the registry out of configuration.
///
/// Every `[assist.NAME]` section becomes an assistant. Sections are discovered
/// rather than listed, so a user-defined one is not a second-class citizen.
pub fn registry(cfg: &Config) -> Vec<Assistant> {
    let mut names: Vec<String> = Vec::new();
    for (key, _) in cfg.keys_under("assist.") {
        // assist.<name>.<field>
        let mut parts = key.splitn(3, '.');
        parts.next();
        if let Some(name) = parts.next() {
            if parts.next().is_some() && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
        }
    }
    names.sort();

    names
        .into_iter()
        .map(|name| {
            let backend = cfg
                .str_or(&format!("assist.{}.backend", name), "anthropic")
                .to_string();
            let model = cfg
                .str_or(&format!("assist.{}.model", name), "")
                .to_string();
            let mut aliases = cfg.list(&format!("assist.{}.aliases", name));
            if !aliases.iter().any(|a| a == &name) {
                aliases.push(name.clone());
            }
            Assistant {
                name,
                backend,
                model,
                aliases,
            }
        })
        .collect()
}

/// Split an utterance into an assistant and the question meant for it.
///
/// Only the first word is considered, and only as a whole word. "claude" alone
/// is not a question, and "the claudel exhibition" is not addressed to anyone.
pub fn address(utterance: &str, assistants: &[Assistant]) -> Option<(Assistant, String)> {
    let trimmed = utterance.trim();
    let (first, rest) = match trimmed.split_once(char::is_whitespace) {
        Some((f, r)) => (f, r.trim()),
        None => (trimmed, ""),
    };
    if rest.is_empty() {
        return None;
    }
    assistants
        .iter()
        .find(|a| a.answers_to(first))
        .map(|a| (a.clone(), rest.to_string()))
}

/// Ask one.
pub fn ask(
    assistant: &Assistant,
    question: &str,
    router: &Router,
    cfg: &Config,
) -> Result<Json, String> {
    let system = cfg.str_or(
        "assist.system",
        "You are answering someone at their computer. Be direct and brief. \
         Plain text, no markdown headings, no preamble.",
    );
    let mut completion = Completion::new(system, question);
    completion.temperature = cfg.f64_or("assist.temperature", 0.4);
    completion.max_tokens = cfg.u64_or("assist.max_tokens", 1024);
    if assistant.is_local() {
        completion.tier = Tier::Large;
    }

    let model = if assistant.model.is_empty() {
        None
    } else {
        Some(assistant.model.as_str())
    };
    let served = router
        .complete_from(&assistant.backend, model, &completion)
        .map_err(|e| {
            format!(
                "{} could not answer: {}. Configure it with `nousctl key set {}`.",
                assistant.name, e, assistant.backend
            )
        })?;

    Ok(json_obj([
        ("assistant", assistant.name.clone().into()),
        ("backend", served.backend.into()),
        ("model", served.model.into()),
        ("question", question.into()),
        ("answer", served.text.into()),
        ("local", assistant.is_local().into()),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Vec<Assistant> {
        registry(&Config::with_defaults())
    }

    #[test]
    fn the_shipped_assistants_are_discovered() {
        let r = defaults();
        let names: Vec<&str> = r.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"claude"), "{names:?}");
        assert!(names.contains(&"chatgpt"), "{names:?}");
        assert!(names.contains(&"local"), "{names:?}");
    }

    #[test]
    fn a_user_defined_assistant_is_not_second_class() {
        let mut cfg = Config::with_defaults();
        cfg.set("assist.mistral.backend", "openai");
        cfg.set("assist.mistral.model", "mistral-large");
        cfg.set("assist.mistral.aliases", "mistral,mi");

        let r = registry(&cfg);
        let m = r
            .iter()
            .find(|a| a.name == "mistral")
            .expect("should be discovered");
        assert_eq!(m.model, "mistral-large");
        assert!(m.answers_to("mi"));
    }

    #[test]
    fn addressing_needs_a_name_and_a_question() {
        let r = defaults();
        let (a, q) = address("claude what is a capability", &r).unwrap();
        assert_eq!(a.name, "claude");
        assert_eq!(q, "what is a capability");

        // Aliases work.
        assert_eq!(address("gpt summarise this", &r).unwrap().0.name, "chatgpt");
        // Punctuation after the name is fine.
        assert_eq!(
            address("claude, what time is it", &r).unwrap().0.name,
            "claude"
        );
    }

    #[test]
    fn a_bare_name_is_not_a_question() {
        let r = defaults();
        assert!(address("claude", &r).is_none());
        assert!(address("   chatgpt   ", &r).is_none());
    }

    #[test]
    fn only_the_first_whole_word_addresses_an_assistant() {
        let r = defaults();
        // Not addressed to anyone: the name is not first.
        assert!(address("ask claude about this", &r).is_none());
        // And a word that merely starts with a name is a different word.
        assert!(address("claudel exhibition opening times", &r).is_none());
        // Ordinary intents are untouched.
        assert!(address("tidy my downloads", &r).is_none());
        assert!(address("open my documents", &r).is_none());
    }

    #[test]
    fn local_assistants_are_distinguished_from_hosted_ones() {
        let r = defaults();
        let local = r.iter().find(|a| a.name == "local").unwrap();
        let claude = r.iter().find(|a| a.name == "claude").unwrap();
        assert!(local.is_local(), "ollama runs here");
        assert!(!claude.is_local(), "a hosted model does not");
        assert!(local.to_json(true).bool_or("local", false));
    }

    #[test]
    fn asking_a_capability_scope_names_the_assistant() {
        // The plan shows `assist.ask:claude`, so which third party is about to
        // receive what you typed is visible before it is sent.
        let r = defaults();
        let (a, _) = address("claude hello", &r).unwrap();
        let cap = format!("assist.ask:{}", a.name);
        let parsed = nous_core::Capability::parse(&cap).unwrap();
        assert_eq!(parsed.scope, "claude");
        assert_eq!(parsed.risk(), nous_core::Risk::Elevated);
    }
}
