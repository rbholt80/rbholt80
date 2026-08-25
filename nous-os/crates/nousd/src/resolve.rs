//! Intent resolution: natural language to a plan.
//!
//! Two resolvers, tried in order.
//!
//! 1. **Grammar.** A deterministic matcher over a few dozen shapes people
//!    actually say to a computer. It is fast, private, free, and it works with
//!    no model installed — which is why it comes first rather than being a
//!    fallback. Opening a folder should not require an inference.
//! 2. **Model.** When the grammar is not confident, a model is asked — and it
//!    answers in [GLYPH](nous_core::glyph), not in prose and not in shell. Its
//!    output is parsed and checked against the capability system before a
//!    single step runs, so a hallucinated command is a syntax error rather than
//!    an incident.

use crate::assist::{self, Assistant};
use crate::router::{Completion, Router};
use nous_core::cap::KNOWN_CAPABILITIES;
use nous_core::glyph::{self, ast, Flow, Stmt, Value};
use nous_core::json::{json_obj, Json};
use nous_core::{Plan, Step};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Bindings produced by steps that have already run.
pub type Env = BTreeMap<String, Json>;

/// What the user was looking at when they asked.
///
/// This is the difference between an assistant you have to describe things to
/// and one that is already in the room. The overlay captures the focused window
/// *before* it appears — otherwise the answer would always be "NOUS" — and the
/// file manager's context menu passes the selection straight through.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// Title of the window that was focused before NOUS was summoned.
    pub focus: Option<String>,
    /// Files selected in the file manager, if that is where this came from.
    pub paths: Vec<PathBuf>,
    /// The directory the user is looking at.
    pub cwd: Option<PathBuf>,
}

impl Context {
    pub fn from_json(v: &Json) -> Context {
        Context {
            focus: v
                .get("focus")
                .and_then(|f| f.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from),
            paths: v
                .str_list("paths")
                .iter()
                .map(|p| nous_core::config::expand_tilde(p))
                .collect(),
            cwd: v
                .get("cwd")
                .and_then(|c| c.as_str())
                .filter(|s| !s.is_empty())
                .map(nous_core::config::expand_tilde),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.focus.is_none() && self.paths.is_empty() && self.cwd.is_none()
    }

    /// A short description for the model prompt.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(f) = &self.focus {
            parts.push(format!("The user is looking at a window titled \"{}\".", f));
        }
        if let Some(c) = &self.cwd {
            parts.push(format!("Their current folder is {}.", c.display()));
        }
        match self.paths.len() {
            0 => {}
            1 => parts.push(format!(
                "They have selected the file {}.",
                self.paths[0].display()
            )),
            n => {
                let shown: Vec<String> = self
                    .paths
                    .iter()
                    .take(6)
                    .map(|p| p.display().to_string())
                    .collect();
                parts.push(format!(
                    "They have selected {} files: {}{}",
                    n,
                    shown.join(", "),
                    if n > 6 { ", and others" } else { "" }
                ));
            }
        }
        parts.join(" ")
    }
}

/// Marker keys used to defer a value until run time.
pub const REF_KEY: &str = "$ref";
pub const FMT_KEY: &str = "$fmt";

/// Steps handled by the daemon itself rather than by an executor. They have no
/// capability because they have no effect on the machine.
pub const HANDLER_FLOW: &str = "flow";

// ------------------------------------------------------- GLYPH -> plan steps

/// Lower a checked flow into executable steps.
///
/// Values that depend on earlier results become `{"$ref": "..."}` markers,
/// resolved by [`resolve_args`] just before the step runs.
pub fn compile_flow(flow: &Flow, platform: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut bound: Vec<String> = Vec::new();
    lower(flow, &flow.stmts, platform, &mut bound, &mut steps);
    steps
}

fn lower(
    flow: &Flow,
    stmts: &[Stmt],
    platform: &str,
    bound: &mut Vec<String>,
    out: &mut Vec<Step>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::On {
                platform: p, body, ..
            } => {
                // Blocks for other platforms are dropped here rather than
                // emitted and skipped, so the plan a user sees is the plan that
                // will run on their machine.
                if p == platform {
                    lower(flow, body, platform, bound, out);
                }
            }
            Stmt::Gate { cond, line } => {
                let id = format!("s{}", out.len() + 1);
                out.push(Step::new(
                    &id,
                    "flow.gate",
                    HANDLER_FLOW,
                    &format!("continue only if {}", cond.render()),
                    json_obj([
                        ("left", encode(&cond.left, bound)),
                        (
                            "op",
                            cond.op
                                .map(|o| Json::Str(o.as_str().to_string()))
                                .unwrap_or(Json::Null),
                        ),
                        (
                            "right",
                            cond.right
                                .as_ref()
                                .map(|r| encode(r, bound))
                                .unwrap_or(Json::Null),
                        ),
                        ("line", (*line as u64).into()),
                    ]),
                ));
            }
            Stmt::Ask { prompt, line } => {
                let id = format!("s{}", out.len() + 1);
                out.push(Step::new(
                    &id,
                    "flow.ask",
                    HANDLER_FLOW,
                    "ask before continuing",
                    json_obj([
                        ("prompt", encode(prompt, bound)),
                        ("line", (*line as u64).into()),
                    ]),
                ));
            }
            Stmt::Bind { name, call } => {
                let mut step = lower_call(flow, call, platform, bound, out.len());
                step.args.set("$bind", Json::Str(name.clone()));
                out.push(step);
                bound.push(name.clone());
            }
            Stmt::Effect(call) => {
                out.push(lower_call(flow, call, platform, bound, out.len()));
            }
        }
    }
}

fn lower_call(
    flow: &Flow,
    call: &ast::Call,
    platform: &str,
    bound: &[String],
    index: usize,
) -> Step {
    let id = format!("s{}", index + 1);

    // A foreign tool becomes a governed shell.exec rather than a special case.
    if let Some(f) = flow.foreign_for(&call.target, platform) {
        let mut argv: Vec<Json> = Vec::new();
        if let Some(Value::List(items)) = call.arg("args") {
            for i in items {
                argv.push(encode(i, bound));
            }
        }
        return Step::new(
            &id,
            &format!("shell.exec:{}", f.cmd),
            "sys",
            &format!("run {}", f.cmd),
            json_obj([
                ("program", f.cmd.clone().into()),
                ("argv", Json::Arr(argv)),
                ("foreign", call.target.clone().into()),
            ]),
        );
    }

    let scope = call
        .scope_value()
        .and_then(|v| v.literal())
        .and_then(|j| j.as_str().map(String::from))
        .unwrap_or_else(|| "*".to_string());
    let capability = if scope == "*" {
        call.target.clone()
    } else {
        format!("{}:{}", call.target, scope)
    };

    let mut args = Json::obj();
    for (k, v) in &call.args {
        args.set(k, encode(v, bound));
    }

    let handler = call.target.split('.').next().unwrap_or("fs").to_string();
    let summary = if call.args.is_empty() {
        call.target.clone()
    } else {
        let rendered: Vec<String> = call
            .args
            .iter()
            .map(|(k, v)| format!("{}: {}", k, v.render()))
            .collect();
        format!("{} {}", call.target, rendered.join(", "))
    };
    Step::new(&id, &capability, &handler, &summary, args)
}

/// Turn a GLYPH value into JSON, deferring anything that needs a prior result.
fn encode(v: &Value, bound: &[String]) -> Json {
    match v {
        Value::Ref(r) => json_obj([(REF_KEY, r.clone().into())]),
        // A bare word is a reference only if it names a binding; otherwise it is
        // a literal symbol like `duplicate`.
        Value::Word(w) if bound.iter().any(|b| b == w) => json_obj([(REF_KEY, w.clone().into())]),
        Value::List(items) => Json::Arr(items.iter().map(|i| encode(i, bound)).collect()),
        Value::Str(pieces) => match glyph::render_literal(pieces) {
            Some(s) => Json::Str(s),
            None => {
                let parts: Vec<Json> = pieces
                    .iter()
                    .map(|p| match p {
                        glyph::Piece::Lit(s) => json_obj([("lit", s.clone().into())]),
                        glyph::Piece::Ref(r) => json_obj([("ref", r.clone().into())]),
                    })
                    .collect();
                json_obj([(FMT_KEY, Json::Arr(parts))])
            }
        },
        other => other.literal().unwrap_or(Json::Null),
    }
}

/// Look up a dotted path in the environment, e.g. `plan.steps`.
pub fn lookup(env: &Env, dotted: &str) -> Option<Json> {
    let (head, rest) = match dotted.split_once('.') {
        Some((h, r)) => (h, Some(r)),
        None => (dotted, None),
    };
    let root = env.get(head)?;
    match rest {
        None => Some(root.clone()),
        Some(path) => root.path(path).cloned(),
    }
}

/// Replace deferred markers with values from `env`.
pub fn resolve_args(args: &Json, env: &Env) -> Result<Json, String> {
    match args {
        Json::Obj(map) => {
            if let Some(Json::Str(r)) = map.get(REF_KEY) {
                return lookup(env, r).ok_or_else(|| {
                    format!(
                        "`{}` is not available — the step that produces it did not run",
                        r
                    )
                });
            }
            if let Some(Json::Arr(parts)) = map.get(FMT_KEY) {
                let mut out = String::new();
                for p in parts {
                    if let Some(l) = p.get("lit").and_then(|v| v.as_str()) {
                        out.push_str(l);
                    } else if let Some(r) = p.get("ref").and_then(|v| v.as_str()) {
                        let v =
                            lookup(env, r).ok_or_else(|| format!("`{}` is not available", r))?;
                        out.push_str(&match v {
                            Json::Str(s) => s,
                            other => other.to_string(),
                        });
                    }
                }
                return Ok(Json::Str(out));
            }
            let mut out = Json::obj();
            for (k, v) in map {
                out.set(k, resolve_args(v, env)?);
            }
            Ok(out)
        }
        Json::Arr(items) => {
            let mut out = Vec::with_capacity(items.len());
            for i in items {
                out.push(resolve_args(i, env)?);
            }
            Ok(Json::Arr(out))
        }
        other => Ok(other.clone()),
    }
}

// ------------------------------------------------------------------- grammar

/// The deterministic resolver.
pub mod grammar {
    use super::*;

    pub struct Ctx {
        pub home: PathBuf,
        /// What the user was looking at when they asked.
        pub context: Context,
    }

    /// Well-known folders, so "downloads" resolves without a model.
    pub fn folder(word: &str, home: &Path) -> Option<PathBuf> {
        let w = word
            .trim()
            .trim_end_matches(&['.', '?', '!'][..])
            .to_ascii_lowercase();
        let name = match w.as_str() {
            "downloads" | "download" => "Downloads",
            "documents" | "docs" | "document" => "Documents",
            "music" | "songs" | "tunes" => "Music",
            "videos" | "movies" | "video" | "films" => "Videos",
            "pictures" | "photos" | "images" | "pics" => "Pictures",
            "desktop" => "Desktop",
            "home" => return Some(home.to_path_buf()),
            "trash" | "bin" => return Some(nous_core::ipc::state_dir().join("trash")),
            _ => return None,
        };
        Some(home.join(name))
    }

    /// Pull a path out of an utterance: an explicit one, or a known folder name.
    pub fn find_path(words: &[String], ctx: &Ctx) -> Option<PathBuf> {
        for w in words {
            if w.starts_with('~') || w.starts_with('/') {
                return Some(nous_core::config::expand_tilde(w));
            }
        }
        words.iter().find_map(|w| folder(w, &ctx.home))
    }

    /// Everything after the first occurrence of any word in `after`.
    pub fn tail_after(words: &[String], after: &[&str]) -> String {
        for (i, w) in words.iter().enumerate() {
            if after.contains(&w.as_str()) {
                return words[i + 1..].join(" ");
            }
        }
        String::new()
    }

    /// Is there an installed application by roughly this name?
    ///
    /// Consulted before claiming an "open X" utterance, so a phrase that merely
    /// begins with a verb is not turned into a launch of something that does
    /// not exist.
    fn app_exists(name: &str) -> bool {
        let dirs: Vec<PathBuf> = ["/usr/share/applications", "/usr/local/share/applications"]
            .iter()
            .map(PathBuf::from)
            .chain(
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".local/share/applications")),
            )
            .collect();
        let lower = name.to_ascii_lowercase();
        crate::exec::desktop::installed_apps(&dirs)
            .iter()
            .any(|a| a.str_or("name", "").to_ascii_lowercase().contains(&lower))
    }

    fn step(n: usize, cap: &str, handler: &str, summary: &str, args: Json) -> Step {
        Step::new(&format!("s{}", n + 1), cap, handler, summary, args)
    }

    fn p(path: &Path) -> Json {
        Json::Str(path.to_string_lossy().to_string())
    }

    /// Does this utterance point at whatever the user has selected?
    fn refers_to_selection(words: &[String]) -> bool {
        words.iter().any(|w| {
            matches!(
                w.trim_end_matches(&['.', '?', '!', ','][..]),
                "this" | "these" | "them" | "their" | "its" | "it" | "selected" | "selection"
            )
        })
    }

    /// Try to resolve deterministically. Returns steps and a confidence.
    pub fn resolve(utterance: &str, ctx: &Ctx) -> Option<(Vec<Step>, f64)> {
        let lower = utterance.to_ascii_lowercase();
        let words: Vec<String> = lower.split_whitespace().map(|s| s.to_string()).collect();
        if words.is_empty() {
            return None;
        }
        let has = |w: &str| {
            words
                .iter()
                .any(|x| x.trim_end_matches(&['.', '?', '!', ','][..]) == w)
        };
        let any = |ws: &[&str]| ws.iter().any(|w| has(w));
        // An explicit path in the words wins; otherwise the folder the user is
        // looking at stands in for one.
        let path = find_path(&words, ctx).or_else(|| ctx.context.cwd.clone());

        // --- what you have selected ----------------------------------------
        // "open these", "delete this", "tidy these" -- the file manager already
        // told us what they are, so there is nothing to guess at.
        if !ctx.context.paths.is_empty()
            && any(&["copy"])
            && any(&["path", "paths", "name", "names"])
        {
            {
                let joined = ctx
                    .context
                    .paths
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                let n = ctx.context.paths.len();
                return Some((
                    vec![step(
                        0,
                        "desk.copy",
                        "desk",
                        &format!("copy {} path(s)", n),
                        json_obj([("text", joined.into())]),
                    )],
                    0.9,
                ));
            }
        }
        if !ctx.context.paths.is_empty() && refers_to_selection(&words) {
            let selected: Vec<Json> = ctx
                .context
                .paths
                .iter()
                .map(|p| Json::Str(p.to_string_lossy().to_string()))
                .collect();
            let n = selected.len();

            if any(&["open", "view", "show"]) {
                let steps: Vec<Step> = ctx
                    .context
                    .paths
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        step(
                            i,
                            &format!("desk.open:{}", p.display()),
                            "desk",
                            &format!(
                                "open {}",
                                p.file_name().and_then(|f| f.to_str()).unwrap_or("?")
                            ),
                            json_obj([("path", self::p(p))]),
                        )
                    })
                    .collect();
                return Some((steps, 0.9));
            }
            if any(&["delete", "remove", "bin", "trash"]) {
                let steps: Vec<Step> = ctx
                    .context
                    .paths
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        step(
                            i,
                            &format!("fs.delete:{}", p.display()),
                            "fs",
                            &format!(
                                "move {} to the trash store",
                                p.file_name().and_then(|f| f.to_str()).unwrap_or("?")
                            ),
                            json_obj([("path", self::p(p))]),
                        )
                    })
                    .collect();
                return Some((steps, 0.88));
            }
            if any(&["tidy", "clean", "organise", "organize", "sort"]) {
                // Several selected files usually share a parent; scanning it
                // once is enough, and twice would double every finding.
                let mut dirs: Vec<PathBuf> = ctx
                    .context
                    .paths
                    .iter()
                    .filter_map(|p| {
                        if p.is_dir() {
                            Some(p.clone())
                        } else {
                            p.parent().map(|d| d.to_path_buf())
                        }
                    })
                    .collect();
                dirs.sort();
                dirs.dedup();
                let roots: Vec<Json> = dirs
                    .iter()
                    .map(|d| Json::Str(d.to_string_lossy().to_string()))
                    .collect();
                let args = json_obj([("roots", Json::Arr(roots))]);
                return Some((
                    vec![
                        step(
                            0,
                            "curate.scan",
                            "curate",
                            "look for things to tidy",
                            args.clone(),
                        ),
                        step(1, "curate.propose", "curate", "work out what to move", args),
                    ],
                    0.87,
                ));
            }
            let _ = (selected, n);
        }

        // --- the ledger ----------------------------------------------------
        if any(&["undo", "revert", "unde"]) {
            return Some((
                vec![step(
                    0,
                    "journal.revert",
                    "journal",
                    "undo the last action",
                    Json::obj(),
                )],
                0.95,
            ));
        }
        if (any(&["what", "which"]) && any(&["did", "done", "changed"]))
            || any(&["history", "ledger", "journal"])
        {
            return Some((
                vec![step(
                    0,
                    "journal.read",
                    "journal",
                    "show recent activity",
                    json_obj([("limit", 25u64.into())]),
                )],
                0.85,
            ));
        }

        // --- tidying -------------------------------------------------------
        if any(&[
            "tidy",
            "clean",
            "cleanup",
            "organise",
            "organize",
            "declutter",
            "sort",
        ]) {
            let roots = match &path {
                Some(p) => Json::Arr(vec![self::p(p)]),
                None => Json::Null,
            };
            let mut args = Json::obj();
            if !roots.is_null() {
                args.set("roots", roots);
            }
            return Some((
                vec![
                    step(
                        0,
                        "curate.scan",
                        "curate",
                        "look for things to tidy",
                        args.clone(),
                    ),
                    step(1, "curate.propose", "curate", "work out what to move", args),
                ],
                0.88,
            ));
        }
        if any(&["duplicate", "duplicates", "dupes", "copies"]) {
            return Some((
                vec![step(
                    0,
                    "curate.scan",
                    "curate",
                    "look for duplicate files",
                    Json::obj(),
                )],
                0.85,
            ));
        }

        // --- machine state -------------------------------------------------
        if any(&["space", "disk", "storage", "full"]) {
            return Some((
                vec![
                    step(0, "sys.metrics", "sys", "check disk usage", Json::obj()),
                    step(
                        1,
                        "curate.scan",
                        "curate",
                        "look for space to reclaim",
                        Json::obj(),
                    ),
                ],
                0.86,
            ));
        }
        if any(&[
            "memory",
            "ram",
            "cpu",
            "load",
            "slow",
            "performance",
            "temperature",
        ]) {
            return Some((
                vec![
                    step(
                        0,
                        "sys.metrics",
                        "sys",
                        "sample machine metrics",
                        Json::obj(),
                    ),
                    step(
                        1,
                        "proc.list",
                        "proc",
                        "list the busiest processes",
                        json_obj([("limit", 10u64.into())]),
                    ),
                ],
                0.85,
            ));
        }
        if any(&["running", "processes", "process"]) {
            return Some((
                vec![step(
                    0,
                    "proc.list",
                    "proc",
                    "list running processes",
                    json_obj([("limit", 25u64.into())]),
                )],
                0.82,
            ));
        }

        // --- playback ------------------------------------------------------
        for (word, action) in [
            ("pause", "pause"),
            ("resume", "resume"),
            ("unpause", "resume"),
            ("skip", "next"),
            ("mute", "volume"),
        ] {
            if has(word) && words.len() <= 3 {
                let mut args = json_obj([("action", action.into())]);
                if action == "volume" {
                    args.set("level", Json::Num(0.0));
                }
                return Some((
                    vec![step(
                        0,
                        "media.control",
                        "media",
                        &format!("{} playback", action),
                        args,
                    )],
                    0.92,
                ));
            }
        }
        if any(&["play", "listen", "watch"]) {
            let query = tail_after(&words, &["play", "listen", "watch"])
                .trim_start_matches("to ")
                .trim()
                .to_string();
            if !query.is_empty() {
                return Some((
                    vec![
                        step(
                            0,
                            "media.search",
                            "media",
                            &format!("find '{}'", query),
                            json_obj([("query", query.clone().into()), ("limit", 1u64.into())]),
                        ),
                        step(1, "media.play", "media", "start playback", Json::obj()),
                    ],
                    0.8,
                ));
            }
        }

        // --- files ---------------------------------------------------------
        if any(&["find", "search", "locate", "where"]) {
            let query = tail_after(&words, &["find", "search", "locate", "where"])
                .replace("for ", "")
                .replace("is ", "")
                .trim()
                .to_string();
            if !query.is_empty() {
                return Some((
                    vec![step(
                        0,
                        "fs.search",
                        "fs",
                        &format!("search for '{}'", query),
                        json_obj([("query", query.into())]),
                    )],
                    0.8,
                ));
            }
        }
        if any(&["show", "list", "open", "browse", "ls"]) {
            if let Some(dir) = path {
                return Some((
                    vec![step(
                        0,
                        &format!("fs.list:{}", dir.display()),
                        "fs",
                        &format!("list {}", dir.display()),
                        json_obj([("path", self::p(&dir))]),
                    )],
                    0.9,
                ));
            }
        }

        // --- the desktop you are already running ---------------------------
        // These sit below the folder rules on purpose: "open my downloads" is a
        // folder, and only an utterance that names no path falls through here.
        if any(&["screenshot", "screengrab", "capture"]) {
            return Some((
                vec![step(
                    0,
                    "desk.screenshot",
                    "desk",
                    "capture the screen",
                    Json::obj(),
                )],
                0.9,
            ));
        }
        if has("lock") && words.len() <= 4 {
            return Some((
                vec![step(
                    0,
                    "desk.session",
                    "desk",
                    "lock the screen",
                    json_obj([("action", "lock".into())]),
                )],
                0.9,
            ));
        }
        if any(&["clipboard"]) {
            return Some((
                vec![step(
                    0,
                    "desk.clipboard",
                    "desk",
                    "read the clipboard",
                    Json::obj(),
                )],
                0.85,
            ));
        }
        if (any(&["what", "which"])
            && any(&["open", "running"])
            && any(&["windows", "window", "apps"]))
            || (any(&["windows"]) && words.len() <= 3)
        {
            return Some((
                vec![step(
                    0,
                    "desk.windows",
                    "desk",
                    "list open windows",
                    Json::obj(),
                )],
                0.85,
            ));
        }
        if any(&["close", "quit"]) {
            let target = tail_after(&words, &["close", "quit"]).trim().to_string();
            if !target.is_empty() {
                return Some((
                    vec![step(
                        0,
                        "desk.close",
                        "desk",
                        &format!("close {}", target),
                        json_obj([("window", target.into())]),
                    )],
                    0.85,
                ));
            }
        }
        if any(&["switch", "focus"]) {
            let target = tail_after(&words, &["switch", "focus"])
                .trim_start_matches("to ")
                .trim()
                .to_string();
            if !target.is_empty() {
                return Some((
                    vec![step(
                        0,
                        "desk.focus",
                        "desk",
                        &format!("switch to {}", target),
                        json_obj([("window", target.into())]),
                    )],
                    0.85,
                ));
            }
        }
        if any(&["launch", "start", "run", "open"]) {
            let name = tail_after(&words, &["launch", "start", "run", "open"])
                .trim_start_matches("up ")
                .trim()
                .to_string();
            // Only claim this if something installed actually matches, so
            // "run the backup" does not become a launch attempt.
            if !name.is_empty() && app_exists(&name) {
                return Some((
                    vec![step(
                        0,
                        &format!("desk.launch:{}", name),
                        "desk",
                        &format!("launch {}", name),
                        json_obj([("name", name.into())]),
                    )],
                    0.85,
                ));
            }
        }

        // --- packages and services -----------------------------------------
        if any(&["install"]) {
            let name = tail_after(&words, &["install"]).trim().to_string();
            if !name.is_empty() {
                return Some((
                    vec![step(
                        0,
                        &format!("pkg.install:{}", name),
                        "pkg",
                        &format!("install {}", name),
                        json_obj([("name", name.into())]),
                    )],
                    0.85,
                ));
            }
        }
        if any(&["restart", "reboot"]) && any(&["service", "daemon", "unit"]) {
            let name = tail_after(&words, &["service", "daemon", "unit"])
                .trim()
                .to_string();
            if !name.is_empty() {
                return Some((
                    vec![step(
                        0,
                        &format!("svc.restart:{}", name),
                        "svc",
                        &format!("restart {}", name),
                        json_obj([("unit", name.into())]),
                    )],
                    0.85,
                ));
            }
        }

        None
    }
}

// ------------------------------------------------------------------ resolver

pub struct Resolver {
    pub grammar_threshold: f64,
    pub home: PathBuf,
    pub max_steps: usize,
    /// Assistants reachable by name from the same box as everything else.
    pub assistants: Vec<Assistant>,
}

impl Resolver {
    pub fn from_config(cfg: &nous_core::Config) -> Resolver {
        Resolver {
            grammar_threshold: cfg.f64_or("plan.grammar_threshold", 0.72),
            home: std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/")),
            max_steps: cfg.u64_or("plan.max_steps", 12) as usize,
            assistants: assist::registry(cfg),
        }
    }

    /// Resolve an utterance into a plan, with no surrounding context.
    pub fn resolve(&self, intent_id: &str, utterance: &str, router: &Router) -> Plan {
        self.resolve_with_context(intent_id, utterance, router, &Context::default())
    }

    /// Resolve an utterance against what the user was looking at.
    pub fn resolve_with_context(
        &self,
        intent_id: &str,
        utterance: &str,
        router: &Router,
        context: &Context,
    ) -> Plan {
        // Addressing an assistant by name comes before every other reading.
        // "claude open my downloads" is a question for Claude, not an
        // instruction to open a folder, and guessing otherwise would be the
        // system talking over you.
        if let Some((assistant, question)) = assist::address(utterance, &self.assistants) {
            let step = Step::new(
                "s1",
                &format!("assist.ask:{}", assistant.name),
                "assist",
                &format!("ask {}", assistant.name),
                json_obj([
                    ("assistant", assistant.name.clone().into()),
                    ("question", question.into()),
                ]),
            );
            return Plan {
                intent_id: intent_id.to_string(),
                utterance: utterance.to_string(),
                steps: vec![step],
                origin: format!("addressed:{}", assistant.name),
                confidence: 1.0,
                clarification: None,
            };
        }

        let ctx = grammar::Ctx {
            home: self.home.clone(),
            context: context.clone(),
        };
        if let Some((steps, confidence)) = grammar::resolve(utterance, &ctx) {
            if confidence >= self.grammar_threshold {
                return Plan {
                    intent_id: intent_id.to_string(),
                    utterance: utterance.to_string(),
                    steps,
                    origin: "grammar".to_string(),
                    confidence,
                    clarification: None,
                };
            }
        }

        match self.resolve_with_model(intent_id, utterance, router, context) {
            Ok(plan) => plan,
            Err(why) => {
                // Fall back to a low-confidence grammar match rather than
                // refusing outright — a plausible guess the user can inspect
                // beats "I could not do that".
                if let Some((steps, confidence)) = grammar::resolve(utterance, &ctx) {
                    return Plan {
                        intent_id: intent_id.to_string(),
                        utterance: utterance.to_string(),
                        steps,
                        origin: "grammar-fallback".to_string(),
                        confidence,
                        clarification: None,
                    };
                }
                Plan::empty(intent_id, utterance, &why)
            }
        }
    }

    fn resolve_with_model(
        &self,
        intent_id: &str,
        utterance: &str,
        router: &Router,
        context: &Context,
    ) -> Result<Plan, String> {
        if !router.has_model() {
            return Err(format!(
                "I could not work out what '{}' means, and no model is available to ask. \
                 Try naming a folder, or run `nousctl models` to configure one.",
                utterance
            ));
        }
        // The model is told what the user is looking at, so "convert these"
        // and "why is this failing" mean something.
        let prompt = if context.is_empty() {
            utterance.to_string()
        } else {
            format!("{}\n\nContext: {}", utterance, context.describe())
        };
        let served = router.complete(&Completion::new(&system_prompt(), &prompt))?;
        let source = extract_glyph(&served.text);
        let program = glyph::parse(&source)
            .map_err(|e| format!("the model produced invalid GLYPH: {}", e))?;
        let flow = program
            .flows
            .first()
            .ok_or_else(|| "the model produced no flow".to_string())?;

        // The check is the safety property: a hallucinated capability is caught
        // here, before anything runs.
        let manifest = glyph::check(flow, ast::current_platform());
        if !manifest.is_valid() {
            let errs: Vec<String> = manifest
                .errors()
                .iter()
                .map(|d| d.message.clone())
                .collect();
            return Err(format!(
                "the model's plan did not check out: {}",
                errs.join("; ")
            ));
        }

        let mut steps = compile_flow(flow, ast::current_platform());
        if steps.len() > self.max_steps {
            steps.truncate(self.max_steps);
        }
        Ok(Plan {
            intent_id: intent_id.to_string(),
            utterance: utterance.to_string(),
            steps,
            origin: format!("model:{}", served.backend),
            // A checked model plan is trustworthy enough to run, but never as
            // trustworthy as an exact grammar match.
            confidence: 0.7,
            clarification: None,
        })
    }
}

/// Pull GLYPH out of a model response, tolerating markdown fences and preamble.
pub fn extract_glyph(text: &str) -> String {
    let t = text.trim();
    if let Some(start) = t.find("```") {
        let after = &t[start + 3..];
        let after = after.strip_prefix("glyph").unwrap_or(after);
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
    }
    // No fence: take from the first `flow` keyword onward.
    match t.find("flow ") {
        Some(i) => t[i..].to_string(),
        None => t.to_string(),
    }
}

/// The system prompt. Generated from the capability registry so it cannot drift
/// away from what the system actually implements.
pub fn system_prompt() -> String {
    let caps = KNOWN_CAPABILITIES
        .iter()
        // Capabilities no flow should ever reach for are omitted rather than
        // listed and forbidden -- do not put the idea in the model's head.
        .filter(|c| {
            !matches!(
                **c,
                "policy.amend" | "secret.read" | "user.admin" | "sys.firmware"
            )
        })
        .map(|c| format!("  {}", c))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"You translate a user's request into GLYPH, the NOUS OS intent language.
Reply with a GLYPH flow and nothing else. No prose, no explanation, no markdown.

GLYPH syntax:
  flow NAME {{
    binding = domain.action arg: value, arg: value
    domain.action arg: value
    gate binding.field > 0
    ask "text with ${{binding.field}}"
  }}

Rules:
- Every statement is a capability call. You may only use the capabilities listed.
- `gate` stops the flow unless the condition holds.
- `ask` requires the human to confirm before anything after it runs.
- Refer to an earlier result with `binding.field`.
- Paths may be written bare: ~/Downloads, /etc/hosts
- Values may be numbers with units (1GB, 30s), strings, lists [a, b], or bare words.
- Put an `ask` before anything that deletes, moves many files, installs software,
  or runs an external program.

Available capabilities:
{}

Example — "clear space in my downloads":
flow tidy-downloads {{
  found = curate.scan    roots: [~/Downloads]
  plan  = curate.propose kinds: [duplicate, misfiled_media]
  gate plan.count > 0
  ask  "Move ${{plan.count}} items out of Downloads?"
  curate.apply steps: plan.steps
}}

Example — "what's eating my memory":
flow check-memory {{
  metrics = sys.metrics
  procs   = proc.list limit: 10
}}"#,
        caps
    )
}

#[cfg(test)]
mod tests {
    use super::grammar::Ctx;
    use super::*;

    fn ctx() -> Ctx {
        Ctx {
            home: PathBuf::from("/home/joey"),
            context: Context::default(),
        }
    }

    fn ctx_with(context: Context) -> Ctx {
        Ctx {
            home: PathBuf::from("/home/joey"),
            context,
        }
    }

    fn resolve(u: &str) -> Option<(Vec<Step>, f64)> {
        grammar::resolve(u, &ctx())
    }

    #[test]
    fn opening_a_folder_needs_no_model() {
        let (steps, conf) = resolve("show me my downloads").unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].capability, "fs.list:/home/joey/Downloads");
        assert!(conf > 0.85, "an exact folder match should be confident");
    }

    #[test]
    fn known_folder_words_all_resolve() {
        for (word, dir) in [
            ("downloads", "Downloads"),
            ("photos", "Pictures"),
            ("movies", "Videos"),
            ("songs", "Music"),
            ("docs", "Documents"),
        ] {
            let got = grammar::folder(word, &PathBuf::from("/home/joey")).unwrap();
            assert!(
                got.ends_with(dir),
                "{} should map to {}, got {:?}",
                word,
                dir,
                got
            );
        }
        assert!(grammar::folder("quux", &PathBuf::from("/home/joey")).is_none());
    }

    #[test]
    fn explicit_paths_win_over_folder_words() {
        let (steps, _) = resolve("list /var/log").unwrap();
        assert_eq!(steps[0].args.str_or("path", ""), "/var/log");
    }

    #[test]
    fn tidying_scans_before_it_proposes() {
        let (steps, _) = resolve("tidy up my downloads").unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].capability, "curate.scan");
        assert_eq!(steps[1].capability, "curate.propose");
        // Crucially, it does not apply anything on its own.
        assert!(!steps
            .iter()
            .any(|s| s.capability.starts_with("curate.apply")));
    }

    #[test]
    fn undo_is_recognised_immediately() {
        let (steps, conf) = resolve("undo").unwrap();
        assert_eq!(steps[0].capability, "journal.revert");
        assert!(conf > 0.9);
    }

    #[test]
    fn asking_what_happened_reads_the_ledger() {
        let (steps, _) = resolve("what did you do").unwrap();
        assert_eq!(steps[0].capability, "journal.read");
    }

    #[test]
    fn machine_questions_sample_metrics_and_processes() {
        let (steps, _) = resolve("why is my computer so slow").unwrap();
        assert_eq!(steps[0].capability, "sys.metrics");
        assert_eq!(steps[1].capability, "proc.list");
    }

    #[test]
    fn playback_control_is_terse_by_design() {
        let (steps, conf) = resolve("pause").unwrap();
        assert_eq!(steps[0].capability, "media.control");
        assert_eq!(steps[0].args.str_or("action", ""), "pause");
        assert!(conf > 0.9);
    }

    #[test]
    fn playing_something_searches_first() {
        let (steps, _) = resolve("play the beatles").unwrap();
        assert_eq!(steps[0].capability, "media.search");
        assert_eq!(steps[0].args.str_or("query", ""), "the beatles");
        assert_eq!(steps[1].capability, "media.play");
    }

    #[test]
    fn installing_extracts_the_package_name() {
        let (steps, _) = resolve("install mpv").unwrap();
        assert_eq!(steps[0].capability, "pkg.install:mpv");
    }

    #[test]
    fn a_selection_makes_this_and_these_mean_something() {
        let dir = std::env::temp_dir().join(format!("nous-sel-res-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("one.txt");
        let b = dir.join("two.txt");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"x").unwrap();

        let context = Context {
            focus: None,
            paths: vec![a.clone(), b.clone()],
            cwd: None,
        };
        let c = ctx_with(context);

        let (steps, _) = grammar::resolve("open these", &c).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(steps[0].capability.starts_with("desk.open:"));

        let (steps, _) = grammar::resolve("delete these", &c).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(steps.iter().all(|s| s.capability.starts_with("fs.delete:")));

        // "copy paths" needs no demonstrative: with a selection in hand it
        // cannot mean anything else.
        // Several files in one folder must not scan that folder twice.
        let (steps, _) = grammar::resolve("tidy these", &c).unwrap();
        let roots = steps[0].args.arr_or_empty("roots");
        assert_eq!(
            roots.len(),
            1,
            "shared parents should collapse: {:?}",
            roots
        );

        let (steps, _) = grammar::resolve("copy the paths", &c).unwrap();
        assert_eq!(steps[0].capability, "desk.copy");
        assert!(steps[0].args.str_or("text", "").contains("one.txt"));
        assert!(steps[0].args.str_or("text", "").contains("two.txt"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_selection_this_and_these_mean_nothing() {
        assert!(grammar::resolve("open these", &ctx()).is_none());
    }

    #[test]
    fn a_named_folder_beats_a_selection() {
        // Files being selected must not turn "open my downloads" into opening
        // those files.
        let context = Context {
            focus: None,
            paths: vec![PathBuf::from("/home/joey/a.txt")],
            cwd: None,
        };
        let (steps, _) = grammar::resolve("open my downloads", &ctx_with(context)).unwrap();
        assert!(
            steps[0].capability.starts_with("fs.list:"),
            "{}",
            steps[0].capability
        );
    }

    #[test]
    fn the_current_folder_stands_in_for_an_unnamed_path() {
        let context = Context {
            focus: None,
            paths: vec![],
            cwd: Some(PathBuf::from("/srv/work")),
        };
        let (steps, _) = grammar::resolve("tidy up", &ctx_with(context)).unwrap();
        assert_eq!(steps[0].capability, "curate.scan");
        assert_eq!(
            steps[0].args.arr_or_empty("roots")[0].as_str(),
            Some("/srv/work")
        );
    }

    #[test]
    fn context_describes_itself_for_the_model() {
        let c = Context {
            focus: Some("report.odt - LibreOffice Writer".into()),
            paths: vec![PathBuf::from("/home/joey/a.png")],
            cwd: Some(PathBuf::from("/home/joey/Pictures")),
        };
        let d = c.describe();
        assert!(d.contains("LibreOffice Writer"), "{d}");
        assert!(d.contains("/home/joey/Pictures"), "{d}");
        assert!(d.contains("a.png"), "{d}");
        assert!(Context::default().is_empty());
    }

    #[test]
    fn addressing_an_assistant_beats_every_other_reading() {
        let cfg = nous_core::Config::with_defaults();
        let resolver = Resolver::from_config(&cfg);
        let router = Router::from_config(&cfg);

        // Without the name this is a folder listing. With it, it is a question.
        let plain = resolver.resolve("i1", "open my downloads", &router);
        assert!(plain.steps[0].capability.starts_with("fs.list:"));

        let addressed = resolver.resolve("i2", "claude open my downloads", &router);
        assert_eq!(addressed.steps[0].capability, "assist.ask:claude");
        assert_eq!(
            addressed.steps[0].args.str_or("question", ""),
            "open my downloads"
        );
        assert_eq!(addressed.origin, "addressed:claude");
    }

    #[test]
    fn desktop_intents_resolve_without_a_model() {
        let (steps, _) = resolve("take a screenshot").unwrap();
        assert_eq!(steps[0].capability, "desk.screenshot");

        let (steps, _) = resolve("lock the screen").unwrap();
        assert_eq!(steps[0].capability, "desk.session");
        assert_eq!(steps[0].args.str_or("action", ""), "lock");

        let (steps, _) = resolve("what windows are open").unwrap();
        assert_eq!(steps[0].capability, "desk.windows");

        let (steps, _) = resolve("close firefox").unwrap();
        assert_eq!(steps[0].capability, "desk.close");
        assert_eq!(steps[0].args.str_or("window", ""), "firefox");

        let (steps, _) = resolve("switch to the terminal").unwrap();
        assert_eq!(steps[0].capability, "desk.focus");
        assert_eq!(steps[0].args.str_or("window", ""), "the terminal");
    }

    #[test]
    fn opening_a_folder_still_beats_launching_an_app() {
        // "open my downloads" must remain a folder listing even though the
        // launch rule also matches the word "open".
        let (steps, _) = resolve("open my downloads").unwrap();
        assert!(
            steps[0].capability.starts_with("fs.list:"),
            "{}",
            steps[0].capability
        );
    }

    #[test]
    fn a_launch_is_only_claimed_for_something_installed() {
        // Nothing called this exists, so the grammar must decline rather than
        // produce a launch that will fail.
        assert!(resolve("open zzzznotaprogram").is_none());
    }

    #[test]
    fn nonsense_falls_through_to_the_model() {
        assert!(resolve("xyzzy plugh").is_none());
        assert!(resolve("").is_none());
    }

    // --- GLYPH lowering ----------------------------------------------------

    fn flow_of(src: &str) -> Flow {
        glyph::parse(src).unwrap().flows.remove(0)
    }

    #[test]
    fn lowers_a_flow_into_steps_with_deferred_references() {
        let f = flow_of(
            r#"flow t {
                 plan = curate.propose kinds: [duplicate]
                 gate plan.count > 0
                 ask "Move ${plan.count}?"
                 curate.apply steps: plan.steps
               }"#,
        );
        let steps = compile_flow(&f, "linux");
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0].args.str_or("$bind", ""), "plan");
        assert_eq!(steps[1].capability, "flow.gate");
        assert_eq!(steps[2].capability, "flow.ask");
        // The reference is deferred, not resolved at compile time.
        assert_eq!(
            steps[3]
                .args
                .path(&format!("steps.{}", REF_KEY))
                .and_then(|v| v.as_str()),
            Some("plan.steps")
        );
    }

    #[test]
    fn resolves_deferred_references_at_run_time() {
        let f = flow_of("flow t { plan = curate.propose\n curate.apply steps: plan.steps }");
        let steps = compile_flow(&f, "linux");
        let mut env = Env::new();
        env.insert(
            "plan".into(),
            json_obj([("steps", Json::Arr(vec![Json::Str("a".into())]))]),
        );

        let resolved = resolve_args(&steps[1].args, &env).unwrap();
        assert_eq!(resolved.arr_or_empty("steps").len(), 1);
    }

    #[test]
    fn an_unavailable_reference_is_a_clear_error() {
        let f = flow_of("flow t { plan = curate.propose\n curate.apply steps: plan.steps }");
        let steps = compile_flow(&f, "linux");
        let err = resolve_args(&steps[1].args, &Env::new()).unwrap_err();
        assert!(err.contains("plan.steps"), "{err}");
        assert!(err.contains("did not run"), "{err}");
    }

    #[test]
    fn interpolated_prompts_are_filled_in_at_run_time() {
        let f = flow_of(
            r#"flow t { plan = curate.propose
                                    ask "Move ${plan.count} files?" }"#,
        );
        let steps = compile_flow(&f, "linux");
        let mut env = Env::new();
        env.insert("plan".into(), json_obj([("count", 7u64.into())]));
        let resolved = resolve_args(&steps[1].args, &env).unwrap();
        assert_eq!(resolved.str_or("prompt", ""), "Move 7 files?");
    }

    #[test]
    fn only_the_running_platforms_blocks_are_lowered() {
        let f = flow_of(
            "flow t {
               on linux   { pkg.install name: mpv }
               on windows { pkg.install name: mpv-win }
             }",
        );
        let steps = compile_flow(&f, "linux");
        assert_eq!(steps.len(), 1, "a plan should show what will actually run");
        assert_eq!(steps[0].capability, "pkg.install:mpv");
    }

    #[test]
    fn foreign_tools_lower_to_governed_shell_steps() {
        let f = flow_of(
            r#"flow t {
                 use foreign handbrake cmd: "HandBrakeCLI"
                 handbrake args: [-i, ~/a.mkv]
               }"#,
        );
        let steps = compile_flow(&f, "linux");
        assert_eq!(steps[0].capability, "shell.exec:HandBrakeCLI");
        assert_eq!(steps[0].args.arr_or_empty("argv").len(), 2);
    }

    #[test]
    fn bare_words_stay_literals_unless_they_name_a_binding() {
        let f = flow_of("flow t { curate.propose kinds: [duplicate] }");
        let steps = compile_flow(&f, "linux");
        let kinds = steps[0].args.arr_or_empty("kinds");
        assert_eq!(
            kinds[0].as_str(),
            Some("duplicate"),
            "a symbol, not a reference"
        );
    }

    // --- model output handling ---------------------------------------------

    #[test]
    fn extracts_glyph_from_a_fenced_response() {
        let text = "Here you go:\n```glyph\nflow t {\n  sys.info\n}\n```\nHope that helps.";
        assert_eq!(extract_glyph(text), "flow t {\n  sys.info\n}");
    }

    #[test]
    fn extracts_glyph_from_an_unfenced_response() {
        let text = "Sure. flow t {\n  sys.info\n}";
        assert!(extract_glyph(text).starts_with("flow t {"));
    }

    #[test]
    fn the_system_prompt_omits_capabilities_no_flow_should_reach_for() {
        let p = system_prompt();
        assert!(p.contains("curate.propose"));
        assert!(p.contains("fs.write"));
        assert!(
            !p.contains("secret.read"),
            "do not name it to the model at all"
        );
        assert!(!p.contains("policy.amend"));
    }

    #[test]
    fn a_model_plan_that_fails_the_check_is_rejected() {
        // The safety property, exercised end to end: invented capabilities do
        // not survive checking, so they never reach an executor.
        let src = "flow bad { fs.incinerate path: /home }";
        let f = flow_of(src);
        let m = glyph::check(&f, "linux");
        assert!(!m.is_valid());
    }
}
