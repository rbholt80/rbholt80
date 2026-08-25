//! Model routing.
//!
//! NOUS treats inference as a system service with several possible providers,
//! tried in order. The ordering matters: a local model is preferred because it
//! is private and free, a hosted one is the fallback, and **no model at all is
//! a supported configuration**. The shell degrades to its deterministic
//! resolver rather than becoming unusable, which is the difference between an
//! AI-native OS and an OS that requires an internet connection to open a folder.

use crate::httpc;
use nous_core::json::{json_obj, Json};
use nous_core::Config;
use nous_core::Secrets;
use std::time::Duration;

/// How much model a request actually needs.
///
/// Most of what an AI-native OS does is small and constant: name this file,
/// classify this download, summarise this folder. Sending that to a paid API
/// would be both expensive and a privacy decision nobody asked for, so it goes
/// to the small local model. `Large` is for the cases that earn it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Small,
    Large,
}

impl Tier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Small => "small",
            Tier::Large => "large",
        }
    }
}

/// A request for text from a model.
#[derive(Debug, Clone)]
pub struct Completion {
    pub system: String,
    pub prompt: String,
    pub max_tokens: u64,
    pub temperature: f64,
    pub tier: Tier,
}

impl Completion {
    /// A large-tier request: intent resolution, anything the user is waiting on.
    pub fn new(system: &str, prompt: &str) -> Completion {
        Completion {
            system: system.to_string(),
            prompt: prompt.to_string(),
            max_tokens: 2048,
            // Low by default: this model's job is to emit a checkable program,
            // not to be interesting.
            temperature: 0.1,
            tier: Tier::Large,
        }
    }

    /// A small-tier request: background classification and housekeeping.
    pub fn small(system: &str, prompt: &str) -> Completion {
        Completion {
            max_tokens: 512,
            tier: Tier::Small,
            ..Completion::new(system, prompt)
        }
    }
}

pub trait Backend: Send + Sync {
    fn name(&self) -> &str;
    /// Cheap liveness check. Must not block for long — it runs on the request path.
    fn available(&self) -> bool;
    fn complete(&self, c: &Completion) -> Result<String, String>;
    fn model(&self) -> String;
}

// ------------------------------------------------------------------- ollama

pub struct Ollama {
    url: String,
    model: String,
    /// The model used for `Tier::Small` requests. Smaller, faster, always local.
    small_model: String,
    timeout: Duration,
}

impl Backend for Ollama {
    fn name(&self) -> &str {
        "ollama"
    }

    fn available(&self) -> bool {
        httpc::reachable(&self.url, Duration::from_millis(250))
    }

    fn model(&self) -> String {
        self.model.clone()
    }

    fn complete(&self, c: &Completion) -> Result<String, String> {
        let model = match c.tier {
            Tier::Small => self.small_model.clone(),
            Tier::Large => self.model.clone(),
        };
        let body = json_obj([
            ("model", model.into()),
            ("prompt", c.prompt.clone().into()),
            ("system", c.system.clone().into()),
            ("stream", false.into()),
            (
                "options",
                json_obj([
                    ("temperature", c.temperature.into()),
                    ("num_predict", c.max_tokens.into()),
                ]),
            ),
        ]);
        let res = httpc::post_json(
            &format!("{}/api/generate", self.url),
            &[],
            &body,
            self.timeout,
        )?;
        let json = res.require_ok()?.json()?;
        let text = json.str_or("response", "");
        if text.is_empty() {
            return Err("ollama returned an empty response".to_string());
        }
        Ok(text.to_string())
    }
}

// ---------------------------------------------------------------- anthropic

pub struct Anthropic {
    url: String,
    model: String,
    max_tokens: u64,
    key_env: String,
    timeout: Duration,
}

impl Anthropic {
    fn key(&self) -> Option<String> {
        // The credential store already overlays the environment, so this covers
        // both a stored key and a one-off `ANTHROPIC_API_KEY=... nsh`.
        Secrets::load()
            .get("anthropic")
            .map(String::from)
            .or_else(|| std::env::var(&self.key_env).ok())
            .filter(|k| !k.trim().is_empty())
    }
}

impl Backend for Anthropic {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn available(&self) -> bool {
        // No key means not configured, which is not an error — it is simply
        // this machine's choice.
        self.key().is_some()
    }

    fn model(&self) -> String {
        self.model.clone()
    }

    fn complete(&self, c: &Completion) -> Result<String, String> {
        let key = self
            .key()
            .ok_or_else(|| format!("{} is not set", self.key_env))?;
        let body = json_obj([
            ("model", self.model.clone().into()),
            ("max_tokens", self.max_tokens.min(c.max_tokens).into()),
            ("temperature", c.temperature.into()),
            ("system", c.system.clone().into()),
            (
                "messages",
                Json::Arr(vec![json_obj([
                    ("role", "user".into()),
                    ("content", c.prompt.clone().into()),
                ])]),
            ),
        ]);
        let res = httpc::post_json(
            &self.url,
            &[
                ("x-api-key", key.as_str()),
                ("anthropic-version", "2023-06-01"),
            ],
            &body,
            self.timeout,
        )?;
        let json = res.require_ok()?.json()?;
        let text = extract_text(&json);
        if text.is_empty() {
            return Err("the model returned no text".to_string());
        }
        Ok(text)
    }
}

/// Pull the text out of a messages-API response, concatenating text blocks.
pub fn extract_text(json: &Json) -> String {
    json.arr_or_empty("content")
        .iter()
        .filter(|b| b.str_or("type", "text") == "text")
        .map(|b| b.str_or("text", "").to_string())
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string()
}

// ----------------------------------------------------- openai-compatible

/// Any endpoint speaking the OpenAI chat-completions shape.
///
/// This is one backend rather than five because OpenAI, OpenRouter, Groq,
/// Together, LM Studio and a local llama.cpp server all speak it. Point the URL
/// wherever your key is good for.
pub struct OpenAICompat {
    url: String,
    model: String,
    /// Which entry in the credential store to use.
    provider: String,
    timeout: Duration,
}

impl Backend for OpenAICompat {
    fn name(&self) -> &str {
        "openai"
    }

    fn available(&self) -> bool {
        // A local OpenAI-compatible server needs no key; a hosted one does.
        let local = self.url.contains("127.0.0.1") || self.url.contains("localhost");
        local || Secrets::load().has(&self.provider)
    }

    fn model(&self) -> String {
        self.model.clone()
    }

    fn complete(&self, c: &Completion) -> Result<String, String> {
        let body = json_obj([
            ("model", self.model.clone().into()),
            ("max_tokens", c.max_tokens.into()),
            ("temperature", c.temperature.into()),
            (
                "messages",
                Json::Arr(vec![
                    json_obj([
                        ("role", "system".into()),
                        ("content", c.system.clone().into()),
                    ]),
                    json_obj([
                        ("role", "user".into()),
                        ("content", c.prompt.clone().into()),
                    ]),
                ]),
            ),
        ]);
        let secrets = Secrets::load();
        let auth = secrets.get(&self.provider).map(|k| format!("Bearer {}", k));
        let headers: Vec<(&str, &str)> = match &auth {
            Some(a) => vec![("Authorization", a.as_str())],
            None => Vec::new(),
        };
        let res = httpc::post_json(&self.url, &headers, &body, self.timeout)?;
        let json = res.require_ok()?.json()?;
        let text = json
            .arr_or_empty("choices")
            .first()
            .and_then(|c| c.path("message.content"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            return Err("the model returned no text".to_string());
        }
        Ok(text)
    }
}

// ------------------------------------------------------------------- router

pub struct Router {
    backends: Vec<Box<dyn Backend>>,
    /// Preference order for large requests. `offline` terminates the search.
    order: Vec<String>,
    /// Preference order for small ones. Defaults to local-only.
    small_order: Vec<String>,
}

/// Which backend served a completion, and what it said.
#[derive(Debug)]
pub struct Served {
    pub backend: String,
    pub model: String,
    pub text: String,
}

impl Router {
    pub fn from_config(cfg: &Config) -> Router {
        let timeout = Duration::from_secs(cfg.u64_or("model.timeout_secs", 60));
        let backends: Vec<Box<dyn Backend>> = vec![
            Box::new(Ollama {
                url: cfg
                    .str_or("model.ollama.url", "http://127.0.0.1:11434")
                    .trim_end_matches('/')
                    .to_string(),
                model: cfg
                    .str_or("model.ollama.model", "qwen2.5:7b-instruct")
                    .to_string(),
                small_model: cfg
                    .str_or("model.ollama.small_model", "qwen2.5:1.5b-instruct")
                    .to_string(),
                timeout,
            }),
            Box::new(Anthropic {
                url: cfg
                    .str_or(
                        "model.anthropic.url",
                        "https://api.anthropic.com/v1/messages",
                    )
                    .to_string(),
                model: cfg
                    .str_or("model.anthropic.model", "claude-sonnet-5")
                    .to_string(),
                max_tokens: cfg.u64_or("model.anthropic.max_tokens", 2048),
                key_env: cfg
                    .str_or("model.anthropic.key_env", "ANTHROPIC_API_KEY")
                    .to_string(),
                timeout,
            }),
            Box::new(OpenAICompat {
                url: cfg
                    .str_or(
                        "model.openai.url",
                        "https://api.openai.com/v1/chat/completions",
                    )
                    .to_string(),
                model: cfg.str_or("model.openai.model", "gpt-4o-mini").to_string(),
                provider: cfg.str_or("model.openai.provider", "openai").to_string(),
                timeout,
            }),
        ];
        let order = cfg.list("model.route");
        let small_order = match cfg.list("model.route.small") {
            v if v.is_empty() => order.clone(),
            v => v,
        };
        Router {
            backends,
            order,
            small_order,
        }
    }

    /// For tests and for embedding a fixed backend.
    pub fn with_backends(backends: Vec<Box<dyn Backend>>, order: Vec<String>) -> Router {
        Router {
            backends,
            small_order: order.clone(),
            order,
        }
    }

    /// Same, with a distinct small-tier route.
    pub fn with_routes(
        backends: Vec<Box<dyn Backend>>,
        order: Vec<String>,
        small_order: Vec<String>,
    ) -> Router {
        Router {
            backends,
            order,
            small_order,
        }
    }

    fn route_for(&self, tier: Tier) -> &[String] {
        match tier {
            Tier::Small => &self.small_order,
            Tier::Large => &self.order,
        }
    }

    fn find(&self, name: &str) -> Option<&dyn Backend> {
        self.backends
            .iter()
            .find(|b| b.name() == name)
            .map(|b| b.as_ref())
    }

    /// Is any model reachable right now?
    ///
    /// Honours the `offline` sentinel the same way `complete` does: a route
    /// that says "stop here" means there is no model, whatever is configured
    /// after it.
    pub fn has_model(&self) -> bool {
        self.order
            .iter()
            .take_while(|n| n.as_str() != "offline")
            .any(|n| self.find(n).is_some_and(|b| b.available()))
    }

    /// Try each configured backend in order. Reaching `offline` in the route
    /// stops the search: it is how an operator says "do not phone home".
    pub fn complete(&self, c: &Completion) -> Result<Served, String> {
        let mut attempts: Vec<String> = Vec::new();
        for name in self.route_for(c.tier) {
            if name == "offline" {
                break;
            }
            let backend = match self.find(name) {
                Some(b) => b,
                None => {
                    attempts.push(format!("{}: not a known backend", name));
                    continue;
                }
            };
            if !backend.available() {
                attempts.push(format!("{}: unavailable", name));
                continue;
            }
            match backend.complete(c) {
                Ok(text) => {
                    return Ok(Served {
                        backend: backend.name().to_string(),
                        model: backend.model(),
                        text,
                    })
                }
                // A backend that is up but failing should not stop the route;
                // the next one may well work.
                Err(e) => attempts.push(format!("{}: {}", name, e)),
            }
        }
        Err(if attempts.is_empty() {
            "no model backend is configured".to_string()
        } else {
            format!("no model backend could answer ({})", attempts.join("; "))
        })
    }

    pub fn status(&self) -> Json {
        let list: Vec<Json> = self
            .backends
            .iter()
            .map(|b| {
                json_obj([
                    ("name", b.name().into()),
                    ("model", b.model().into()),
                    ("available", b.available().into()),
                    (
                        "position",
                        self.order
                            .iter()
                            .position(|n| n == b.name())
                            .map(|p| Json::from(p as u64))
                            .unwrap_or(Json::Null),
                    ),
                ])
            })
            .collect();
        json_obj([
            (
                "route",
                Json::Arr(self.order.iter().map(|s| Json::Str(s.clone())).collect()),
            ),
            (
                "route_small",
                Json::Arr(
                    self.small_order
                        .iter()
                        .map(|s| Json::Str(s.clone()))
                        .collect(),
                ),
            ),
            ("backends", Json::Arr(list)),
            ("has_model", self.has_model().into()),
            ("credentials", Secrets::load().status()),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Fake {
        name: &'static str,
        up: bool,
        answer: Result<String, String>,
        calls: Arc<AtomicUsize>,
    }

    impl Backend for Fake {
        fn name(&self) -> &str {
            self.name
        }
        fn available(&self) -> bool {
            self.up
        }
        fn model(&self) -> String {
            format!("{}-model", self.name)
        }
        fn complete(&self, _c: &Completion) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.answer.clone()
        }
    }

    fn fake(
        name: &'static str,
        up: bool,
        answer: Result<String, String>,
    ) -> (Box<dyn Backend>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Box::new(Fake {
                name,
                up,
                answer,
                calls: calls.clone(),
            }),
            calls,
        )
    }

    #[test]
    fn prefers_the_first_available_backend() {
        let (a, a_calls) = fake("ollama", true, Ok("local answer".into()));
        let (b, b_calls) = fake("anthropic", true, Ok("remote answer".into()));
        let r = Router::with_backends(vec![a, b], vec!["ollama".into(), "anthropic".into()]);
        let served = r.complete(&Completion::new("s", "p")).unwrap();
        assert_eq!(served.backend, "ollama");
        assert_eq!(served.text, "local answer");
        assert_eq!(a_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            b_calls.load(Ordering::Relaxed),
            0,
            "the fallback must not be called"
        );
    }

    #[test]
    fn skips_an_unavailable_backend() {
        let (a, a_calls) = fake("ollama", false, Ok("local".into()));
        let (b, _) = fake("anthropic", true, Ok("remote".into()));
        let r = Router::with_backends(vec![a, b], vec!["ollama".into(), "anthropic".into()]);
        assert_eq!(
            r.complete(&Completion::new("s", "p")).unwrap().backend,
            "anthropic"
        );
        assert_eq!(a_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn falls_through_a_backend_that_is_up_but_failing() {
        let (a, _) = fake("ollama", true, Err("model not pulled".into()));
        let (b, _) = fake("anthropic", true, Ok("remote".into()));
        let r = Router::with_backends(vec![a, b], vec!["ollama".into(), "anthropic".into()]);
        assert_eq!(
            r.complete(&Completion::new("s", "p")).unwrap().text,
            "remote"
        );
    }

    #[test]
    fn offline_in_the_route_stops_the_search() {
        let (a, _) = fake("ollama", false, Ok("local".into()));
        let (b, b_calls) = fake("anthropic", true, Ok("remote".into()));
        let r = Router::with_backends(
            vec![a, b],
            vec!["ollama".into(), "offline".into(), "anthropic".into()],
        );
        assert!(r.complete(&Completion::new("s", "p")).is_err());
        assert_eq!(
            b_calls.load(Ordering::Relaxed),
            0,
            "`offline` must mean no request leaves the machine"
        );
        assert!(!r.has_model());
    }

    #[test]
    fn reports_why_every_backend_failed() {
        let (a, _) = fake("ollama", true, Err("connection refused".into()));
        let r = Router::with_backends(vec![a], vec!["ollama".into()]);
        let err = r.complete(&Completion::new("s", "p")).unwrap_err();
        assert!(err.contains("connection refused"), "{err}");
    }

    #[test]
    fn has_model_ignores_the_offline_sentinel() {
        let (a, _) = fake("ollama", true, Ok("x".into()));
        let r = Router::with_backends(vec![a], vec!["ollama".into()]);
        assert!(r.has_model());

        let (b, _) = fake("ollama", false, Ok("x".into()));
        let r2 = Router::with_backends(vec![b], vec!["ollama".into(), "offline".into()]);
        assert!(!r2.has_model(), "an unreachable local model is not a model");
    }

    #[test]
    fn small_requests_use_the_small_route() {
        let (local, local_calls) = fake("ollama", true, Ok("local".into()));
        let (cloud, cloud_calls) = fake("anthropic", true, Ok("cloud".into()));
        let r = Router::with_routes(
            vec![local, cloud],
            vec!["anthropic".into(), "ollama".into()],
            vec!["ollama".into(), "offline".into()],
        );

        let mut small = Completion::small("s", "classify this download");
        small.tier = Tier::Small;
        assert_eq!(r.complete(&small).unwrap().backend, "ollama");
        assert_eq!(
            cloud_calls.load(Ordering::Relaxed),
            0,
            "routine background work must not reach a paid API"
        );

        assert_eq!(
            r.complete(&Completion::new("s", "hard intent"))
                .unwrap()
                .backend,
            "anthropic"
        );
        assert_eq!(local_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_small_route_ending_offline_stays_on_the_machine() {
        let (local, _) = fake("ollama", false, Ok("local".into()));
        let (cloud, cloud_calls) = fake("anthropic", true, Ok("cloud".into()));
        let r = Router::with_routes(
            vec![local, cloud],
            vec!["anthropic".into()],
            vec!["ollama".into(), "offline".into()],
        );
        assert!(r.complete(&Completion::small("s", "p")).is_err());
        assert_eq!(cloud_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn status_reports_the_route_and_each_backend() {
        let (a, _) = fake("ollama", true, Ok("x".into()));
        let r = Router::with_backends(vec![a], vec!["ollama".into(), "offline".into()]);
        let s = r.status();
        assert_eq!(s.arr_or_empty("route").len(), 2);
        assert!(s.bool_or("has_model", false));
        assert_eq!(
            s.arr_or_empty("backends")[0].str_or("model", ""),
            "ollama-model"
        );
    }

    #[test]
    fn extracts_text_from_a_messages_response() {
        let json = json_obj([(
            "content",
            Json::Arr(vec![
                json_obj([("type", "text".into()), ("text", "flow a {".into())]),
                json_obj([("type", "text".into()), ("text", " }".into())]),
            ]),
        )]);
        assert_eq!(extract_text(&json), "flow a { }");
        assert_eq!(extract_text(&Json::obj()), "");
    }
}
