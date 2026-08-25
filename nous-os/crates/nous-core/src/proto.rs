//! Wire protocol.
//!
//! Newline-delimited JSON over a unix socket. The framing is deliberately dull:
//! you can drive the whole system from `socat` or a shell script, which matters
//! for an OS component that has to be debuggable when the graphical session is
//! the thing that is broken.

use crate::json::{json_obj, Json};

pub const PROTO_VERSION: u64 = 1;

/// Methods the daemon answers. Kept as constants so client and server cannot
/// drift on a typo.
pub mod method {
    /// Turn a natural-language utterance into a plan, and run it.
    pub const INTENT_SUBMIT: &str = "intent.submit";
    /// Plan only — resolve the utterance and return the steps without acting.
    pub const INTENT_PLAN: &str = "intent.plan";
    /// Answer a pending confirmation.
    pub const INTENT_CONFIRM: &str = "intent.confirm";

    /// Ask the broker to adjudicate a capability without exercising it.
    pub const CAP_CHECK: &str = "cap.check";

    pub const JOURNAL_TAIL: &str = "journal.tail";
    pub const JOURNAL_REVERT: &str = "journal.revert";

    /// Recall from the context kernel.
    pub const CTX_QUERY: &str = "ctx.query";
    /// Commit something to the context kernel.
    pub const CTX_NOTE: &str = "ctx.note";
    /// The current working set: what the system believes you are doing.
    pub const CTX_FOCUS: &str = "ctx.focus";

    pub const AGENT_LIST: &str = "agent.list";
    pub const AGENT_REGISTER: &str = "agent.register";
    pub const AGENT_INVOKE: &str = "agent.invoke";

    pub const SYS_STATUS: &str = "sys.status";
    pub const SYS_SHUTDOWN: &str = "sys.shutdown";

    /// Raw completion through the model router.
    pub const MODEL_COMPLETE: &str = "model.complete";
    pub const MODEL_STATUS: &str = "model.status";

    /// Semantic file search.
    pub const FS_SEARCH: &str = "fs.search";
    pub const FS_INDEX: &str = "fs.index";

    /// Subscribe this connection to an event topic.
    pub const SUBSCRIBE: &str = "subscribe";

    pub const PING: &str = "ping";
}

/// Event topics published on the bus.
pub mod topic {
    pub const INTENT: &str = "intent";
    pub const CAPABILITY: &str = "capability";
    pub const AGENT: &str = "agent";
    pub const SENSOR: &str = "sensor";
    pub const NOTIFY: &str = "notify";
    pub const LOG: &str = "log";
}

/// Stable error codes. Clients branch on these, never on message text.
pub mod errcode {
    pub const BAD_REQUEST: &str = "bad_request";
    pub const UNKNOWN_METHOD: &str = "unknown_method";
    pub const DENIED: &str = "denied";
    pub const NEEDS_CONFIRMATION: &str = "needs_confirmation";
    pub const NOT_FOUND: &str = "not_found";
    pub const BACKEND_UNAVAILABLE: &str = "backend_unavailable";
    pub const INTERNAL: &str = "internal";
    pub const TOO_LARGE: &str = "too_large";
}

#[derive(Debug, Clone)]
pub struct Request {
    pub id: String,
    pub method: String,
    pub params: Json,
}

impl Request {
    pub fn new(id: &str, method: &str, params: Json) -> Request {
        Request {
            id: id.to_string(),
            method: method.to_string(),
            params,
        }
    }

    pub fn to_json(&self) -> Json {
        json_obj([
            ("v", PROTO_VERSION.into()),
            ("kind", "req".into()),
            ("id", self.id.clone().into()),
            ("method", self.method.clone().into()),
            ("params", self.params.clone()),
        ])
    }

    pub fn from_json(v: &Json) -> Result<Request, String> {
        let ver = v.get("v").and_then(|x| x.as_u64()).unwrap_or(PROTO_VERSION);
        if ver != PROTO_VERSION {
            return Err(format!(
                "unsupported protocol version {} (want {})",
                ver, PROTO_VERSION
            ));
        }
        let method = v
            .get("method")
            .and_then(|m| m.as_str())
            .ok_or("request has no method")?;
        Ok(Request {
            id: v.str_or("id", "").to_string(),
            method: method.to_string(),
            params: v.get("params").cloned().unwrap_or_else(Json::obj),
        })
    }

    pub fn param_str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(|v| v.as_str())
    }

    pub fn param_bool(&self, key: &str, default: bool) -> bool {
        self.params.bool_or(key, default)
    }

    pub fn param_u64(&self, key: &str, default: u64) -> u64 {
        self.params
            .get(key)
            .and_then(|v| v.as_u64())
            .unwrap_or(default)
    }
}

#[derive(Debug, Clone)]
pub enum Response {
    Ok {
        id: String,
        result: Json,
    },
    Err {
        id: String,
        code: String,
        message: String,
        data: Json,
    },
}

impl Response {
    pub fn ok(id: &str, result: Json) -> Response {
        Response::Ok {
            id: id.to_string(),
            result,
        }
    }

    pub fn err(id: &str, code: &str, message: impl Into<String>) -> Response {
        Response::Err {
            id: id.to_string(),
            code: code.to_string(),
            message: message.into(),
            data: Json::Null,
        }
    }

    pub fn err_with(id: &str, code: &str, message: impl Into<String>, data: Json) -> Response {
        Response::Err {
            id: id.to_string(),
            code: code.to_string(),
            message: message.into(),
            data,
        }
    }

    pub fn to_json(&self) -> Json {
        match self {
            Response::Ok { id, result } => json_obj([
                ("v", PROTO_VERSION.into()),
                ("kind", "res".into()),
                ("id", id.clone().into()),
                ("ok", true.into()),
                ("result", result.clone()),
            ]),
            Response::Err {
                id,
                code,
                message,
                data,
            } => json_obj([
                ("v", PROTO_VERSION.into()),
                ("kind", "res".into()),
                ("id", id.clone().into()),
                ("ok", false.into()),
                (
                    "error",
                    json_obj([
                        ("code", code.clone().into()),
                        ("message", message.clone().into()),
                        ("data", data.clone()),
                    ]),
                ),
            ]),
        }
    }

    pub fn from_json(v: &Json) -> Result<Response, String> {
        let id = v.str_or("id", "").to_string();
        if v.bool_or("ok", false) {
            Ok(Response::Ok {
                id,
                result: v.get("result").cloned().unwrap_or(Json::Null),
            })
        } else {
            let e = v.get("error").cloned().unwrap_or_else(Json::obj);
            Ok(Response::Err {
                id,
                code: e.str_or("code", errcode::INTERNAL).to_string(),
                message: e.str_or("message", "unspecified error").to_string(),
                data: e.get("data").cloned().unwrap_or(Json::Null),
            })
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Response::Ok { .. })
    }

    pub fn id(&self) -> &str {
        match self {
            Response::Ok { id, .. } | Response::Err { id, .. } => id,
        }
    }

    /// Unwrap to a result, discarding the id.
    pub fn into_result(self) -> Result<Json, (String, String)> {
        match self {
            Response::Ok { result, .. } => Ok(result),
            Response::Err { code, message, .. } => Err((code, message)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Event {
    pub topic: String,
    pub data: Json,
}

impl Event {
    pub fn new(topic: &str, data: Json) -> Event {
        Event {
            topic: topic.to_string(),
            data,
        }
    }

    pub fn to_json(&self) -> Json {
        json_obj([
            ("v", PROTO_VERSION.into()),
            ("kind", "evt".into()),
            ("topic", self.topic.clone().into()),
            ("data", self.data.clone()),
        ])
    }

    pub fn from_json(v: &Json) -> Event {
        Event {
            topic: v.str_or("topic", "").to_string(),
            data: v.get("data").cloned().unwrap_or(Json::Null),
        }
    }
}

/// Anything that arrives on a connection.
#[derive(Debug, Clone)]
pub enum Frame {
    Req(Request),
    Res(Response),
    Evt(Event),
}

impl Frame {
    pub fn parse(v: &Json) -> Result<Frame, String> {
        match v.str_or("kind", "") {
            "req" => Ok(Frame::Req(Request::from_json(v)?)),
            "res" => Ok(Frame::Res(Response::from_json(v)?)),
            "evt" => Ok(Frame::Evt(Event::from_json(v))),
            other => Err(format!("unknown frame kind '{}'", other)),
        }
    }

    pub fn to_json(&self) -> Json {
        match self {
            Frame::Req(r) => r.to_json(),
            Frame::Res(r) => r.to_json(),
            Frame::Evt(e) => e.to_json(),
        }
    }
}

// ------------------------------------------------------------ intents & plans

/// One step of a plan: a capability plus the arguments to exercise it with.
#[derive(Debug, Clone)]
pub struct Step {
    pub id: String,
    /// The capability string, e.g. `fs.read:/home/joey/notes.md`.
    pub capability: String,
    /// Which handler runs it (`fs`, `sys`, an agent id, ...).
    pub handler: String,
    pub args: Json,
    /// What this step does, in plain language, for the confirmation prompt.
    pub summary: String,
}

impl Step {
    pub fn new(id: &str, capability: &str, handler: &str, summary: &str, args: Json) -> Step {
        Step {
            id: id.to_string(),
            capability: capability.to_string(),
            handler: handler.to_string(),
            args,
            summary: summary.to_string(),
        }
    }

    pub fn to_json(&self) -> Json {
        json_obj([
            ("id", self.id.clone().into()),
            ("capability", self.capability.clone().into()),
            ("handler", self.handler.clone().into()),
            ("summary", self.summary.clone().into()),
            ("args", self.args.clone()),
        ])
    }

    pub fn from_json(v: &Json) -> Step {
        Step {
            id: v.str_or("id", "").to_string(),
            capability: v.str_or("capability", "").to_string(),
            handler: v.str_or("handler", "").to_string(),
            args: v.get("args").cloned().unwrap_or_else(Json::obj),
            summary: v.str_or("summary", "").to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub intent_id: String,
    pub utterance: String,
    pub steps: Vec<Step>,
    /// Which resolver produced this: `grammar`, `model:<backend>`, ...
    pub origin: String,
    /// 0.0–1.0. Low confidence makes the shell show the plan before running it.
    pub confidence: f64,
    /// Set when the system could not work out what was meant.
    pub clarification: Option<String>,
}

impl Plan {
    pub fn empty(intent_id: &str, utterance: &str, clarification: &str) -> Plan {
        Plan {
            intent_id: intent_id.to_string(),
            utterance: utterance.to_string(),
            steps: Vec::new(),
            origin: "none".to_string(),
            confidence: 0.0,
            clarification: Some(clarification.to_string()),
        }
    }

    pub fn to_json(&self) -> Json {
        json_obj([
            ("intent_id", self.intent_id.clone().into()),
            ("utterance", self.utterance.clone().into()),
            ("origin", self.origin.clone().into()),
            ("confidence", self.confidence.into()),
            (
                "steps",
                Json::Arr(self.steps.iter().map(|s| s.to_json()).collect()),
            ),
            (
                "clarification",
                self.clarification
                    .clone()
                    .map(Json::Str)
                    .unwrap_or(Json::Null),
            ),
        ])
    }

    pub fn from_json(v: &Json) -> Plan {
        Plan {
            intent_id: v.str_or("intent_id", "").to_string(),
            utterance: v.str_or("utterance", "").to_string(),
            steps: v
                .arr_or_empty("steps")
                .iter()
                .map(Step::from_json)
                .collect(),
            origin: v.str_or("origin", "unknown").to_string(),
            confidence: v.f64_or("confidence", 0.0),
            clarification: v
                .get("clarification")
                .and_then(|c| c.as_str())
                .map(String::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    #[test]
    fn requests_round_trip() {
        let r = Request::new(
            "1",
            method::INTENT_SUBMIT,
            json_obj([("text", "hello".into())]),
        );
        let back = Request::from_json(&parse(&r.to_json().to_string()).unwrap()).unwrap();
        assert_eq!(back.method, method::INTENT_SUBMIT);
        assert_eq!(back.param_str("text"), Some("hello"));
    }

    #[test]
    fn rejects_a_future_protocol_version() {
        let mut v = Request::new("1", "ping", Json::obj()).to_json();
        v.set("v", Json::Num(99.0));
        assert!(Request::from_json(&v).is_err());
    }

    #[test]
    fn error_responses_carry_a_stable_code() {
        let e = Response::err("7", errcode::DENIED, "policy said no");
        let back = Response::from_json(&parse(&e.to_json().to_string()).unwrap()).unwrap();
        assert!(!back.is_ok());
        match back.into_result() {
            Err((code, msg)) => {
                assert_eq!(code, errcode::DENIED);
                assert_eq!(msg, "policy said no");
            }
            Ok(_) => panic!("expected an error"),
        }
    }

    #[test]
    fn frames_dispatch_on_kind() {
        let cases = [
            Frame::Req(Request::new("1", "ping", Json::obj())),
            Frame::Res(Response::ok("1", Json::Null)),
            Frame::Evt(Event::new(topic::NOTIFY, json_obj([("m", "hi".into())]))),
        ];
        for f in cases {
            let parsed = Frame::parse(&parse(&f.to_json().to_string()).unwrap()).unwrap();
            assert_eq!(
                std::mem::discriminant(&parsed),
                std::mem::discriminant(&f),
                "frame kind changed across a round trip"
            );
        }
        assert!(Frame::parse(&parse(r#"{"kind":"nope"}"#).unwrap()).is_err());
    }

    #[test]
    fn plans_round_trip_with_their_steps() {
        let plan = Plan {
            intent_id: "i1".into(),
            utterance: "tidy my downloads".into(),
            steps: vec![Step::new(
                "s1",
                "fs.list:/home/joey/Downloads",
                "fs",
                "list Downloads",
                json_obj([("path", "/home/joey/Downloads".into())]),
            )],
            origin: "grammar".into(),
            confidence: 0.9,
            clarification: None,
        };
        let back = Plan::from_json(&parse(&plan.to_json().to_string()).unwrap());
        assert_eq!(back.steps.len(), 1);
        assert_eq!(back.steps[0].capability, "fs.list:/home/joey/Downloads");
        assert_eq!(back.confidence, 0.9);
        assert!(back.clarification.is_none());
    }

    #[test]
    fn serialized_frames_never_contain_a_raw_newline() {
        // The transport is newline-delimited, so this is a framing invariant.
        let r = Request::new(
            "1",
            "ctx.note",
            json_obj([("text", "line one\nline two".into())]),
        );
        let wire = r.to_json().to_string();
        assert!(!wire.contains('\n'), "frame must be a single line: {wire}");
        let back = Request::from_json(&parse(&wire).unwrap()).unwrap();
        assert_eq!(back.param_str("text"), Some("line one\nline two"));
    }
}
