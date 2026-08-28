//! `warrant` — the guard as a process.
//!
//! Three of the four things this was extracted for are not written in Rust, so
//! the library alone would have been useless to them. This binary speaks
//! line-delimited JSON on stdin and stdout: one request object per line, one
//! response object per line, in order. That is enough for Python, Kotlin, Node
//! or a shell script to hold a guard without an FFI boundary, and it is the
//! same shape as an MCP server or a Claude Code hook.
//!
//! ```text
//! warrant init      ~/.config/myapp         write a starting policy and grades
//! warrant check     agent:claude fs.delete:/tmp/x    one question, exit code answers
//! warrant history   --limit 20              what has happened
//! warrant serve                             the JSON protocol on stdin/stdout
//! ```

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use warrant::json::{json_obj, parse, Json};
use warrant::{
    Capability, Grades, Guard, Journal, Outcome, Policy, Record, Subject, Undo, EXAMPLE_GRADES,
    EXAMPLE_POLICY, VERSION,
};

const USAGE: &str = "\
warrant — decide what an agent may do, and record how to take it back.

  warrant init [DIR]                    write a starting policy and grades
  warrant check SUBJECT CAPABILITY      adjudicate one request
  warrant history [--limit N]           what has happened
  warrant unfinished                    actions begun and never completed
  warrant serve                         line-delimited JSON on stdin/stdout

Options:
  --dir DIR         where the journal lives      (default ~/.local/share/warrant)
  --policy FILE     policy document              (default DIR/policy.warrant)
  --grades FILE     risk table                   (default DIR/grades.warrant)
  --home PATH       what `~` means in a scope    (default $HOME)
  --json            machine-readable output for check/history
  -h, --help        this
  -V, --version

`check` exits 0 for allow, 10 for confirm, 20 for deny, 30 for never,
64 for a malformed request — so a shell script or a tool hook can branch on it.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("warrant: {}", e);
            ExitCode::from(64)
        }
    }
}

struct Opts {
    dir: PathBuf,
    policy: Option<PathBuf>,
    grades: Option<PathBuf>,
    home: String,
    json: bool,
    limit: usize,
    rest: Vec<String>,
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let mut o = Opts {
        dir: PathBuf::from(&home).join(".local/share/warrant"),
        policy: None,
        grades: None,
        home,
        json: false,
        limit: 20,
        rest: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let mut want = |name: &str| -> Result<String, String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{} needs a value", name))
        };
        match a {
            "--dir" => o.dir = PathBuf::from(want("--dir")?),
            "--policy" => o.policy = Some(PathBuf::from(want("--policy")?)),
            "--grades" => o.grades = Some(PathBuf::from(want("--grades")?)),
            "--home" => o.home = want("--home")?,
            "--limit" => {
                o.limit = want("--limit")?
                    .parse()
                    .map_err(|_| "--limit wants a number".to_string())?
            }
            "--json" => o.json = true,
            other => o.rest.push(other.to_string()),
        }
        i += 1;
    }
    Ok(o)
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    if args.iter().any(|a| a == "-h" || a == "--help") || args.is_empty() {
        print!("{}", USAGE);
        return Ok(ExitCode::SUCCESS);
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("warrant {}", VERSION);
        return Ok(ExitCode::SUCCESS);
    }

    let o = parse_opts(args)?;
    let verb = o.rest.first().cloned().unwrap_or_default();

    match verb.as_str() {
        "init" => {
            let dir = o
                .rest
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| o.dir.clone());
            init(&dir)
        }
        "check" => {
            let subject = o.rest.get(1).ok_or("check needs a subject")?;
            let cap = o.rest.get(2).ok_or("check needs a capability")?;
            check(&o, subject, cap)
        }
        "history" => history(&o),
        "unfinished" => unfinished(&o),
        "serve" => serve(&o),
        other => Err(format!("unknown command '{}' — try --help", other)),
    }
}

fn init(dir: &Path) -> Result<ExitCode, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    for (name, body) in [
        ("policy.warrant", EXAMPLE_POLICY),
        ("grades.warrant", EXAMPLE_GRADES),
    ] {
        let path = dir.join(name);
        if path.exists() {
            eprintln!("warrant: {} already exists, left alone", path.display());
            continue;
        }
        std::fs::write(&path, body).map_err(|e| format!("{}: {}", path.display(), e))?;
        println!("wrote {}", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn load(o: &Opts) -> Result<Guard, String> {
    let policy_path = o
        .policy
        .clone()
        .unwrap_or_else(|| o.dir.join("policy.warrant"));
    let grades_path = o
        .grades
        .clone()
        .unwrap_or_else(|| o.dir.join("grades.warrant"));

    let read = |p: &Path| -> Result<String, String> {
        std::fs::read_to_string(p).map_err(|e| {
            format!(
                "{}: {} — run `warrant init` to write a starting one",
                p.display(),
                e
            )
        })
    };
    let policy = Policy::parse(&read(&policy_path)?, &policy_path.display().to_string())?;
    let grades = Grades::parse(&read(&grades_path)?, &grades_path.display().to_string())?;
    let journal = Journal::open(&o.dir)?;
    Ok(Guard::new(policy, grades, journal, &o.home))
}

fn check(o: &Opts, subject: &str, cap: &str) -> Result<ExitCode, String> {
    let g = load(o)?;
    let cap = Capability::parse(cap)?;
    let r = g.rule(&Subject::parse(subject), &cap);

    if o.json {
        println!("{}", ruling_json(&r));
    } else {
        println!("{}", r.explain());
    }
    Ok(ExitCode::from(if r.forbidden() {
        30
    } else if r.refused() {
        20
    } else if r.needs_confirmation() {
        10
    } else {
        0
    }))
}

fn history(o: &Opts) -> Result<ExitCode, String> {
    let g = load(o)?;
    show(&g.history(o.limit)?, o.json);
    Ok(ExitCode::SUCCESS)
}

fn unfinished(o: &Opts) -> Result<ExitCode, String> {
    let g = load(o)?;
    let open = g.unfinished()?;
    if open.is_empty() && !o.json {
        println!("nothing was left half-done");
        return Ok(ExitCode::SUCCESS);
    }
    show(&open, o.json);
    Ok(ExitCode::SUCCESS)
}

fn show(records: &[Record], as_json: bool) {
    if as_json {
        println!(
            "{}",
            Json::Arr(records.iter().map(|r| r.to_json()).collect())
        );
    } else {
        for r in records {
            println!("{}", r.summary());
        }
    }
}

fn ruling_json(r: &warrant::Ruling) -> Json {
    json_obj([
        ("ok", true.into()),
        ("decision", r.verdict.decision.kind().into()),
        ("reason", r.verdict.decision.reason().into()),
        ("risk", r.verdict.risk.as_str().into()),
        (
            "matched",
            r.verdict
                .matched
                .clone()
                .map(Json::Str)
                .unwrap_or(Json::Null),
        ),
        ("absolute", r.verdict.absolute.into()),
        ("allowed", r.allowed().into()),
        ("explain", r.explain().into()),
        ("prompt", r.prompt().into()),
    ])
}

fn err(msg: &str) -> Json {
    json_obj([("ok", false.into()), ("error", msg.into())])
}

/// One request object per line in, one response object per line out.
///
/// Every response carries `ok`. A failed request is a response, not a dropped
/// connection: a host driving this over a pipe must never have to guess whether
/// silence meant refusal or a crash.
///
/// A request must be *one* line. A pretty-printed object is several lines, and
/// so is several requests — each gets its own error reply, and a caller reading
/// one reply per request is then desynchronised for the rest of the session.
/// Every standard JSON encoder emits one line by default; do not turn indenting
/// on for the wire.
fn serve(o: &Opts) -> Result<ExitCode, String> {
    let g = load(o)?;
    let stdin = io::stdin();
    let mut out = io::stdout();

    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match parse(&line) {
            Ok(req) => handle(&g, &req),
            Err(e) => err(&format!("bad JSON: {}", e)),
        };
        writeln!(out, "{}", reply).map_err(|e| e.to_string())?;
        // Flushed per line: the caller is blocked reading this, and a buffered
        // reply that arrives at exit is a deadlock, not a slow answer.
        out.flush().map_err(|e| e.to_string())?;
    }
    Ok(ExitCode::SUCCESS)
}

fn handle(g: &Guard, req: &Json) -> Json {
    match do_handle(g, req) {
        Ok(v) => v,
        Err(e) => err(&e),
    }
}

fn want_cap(req: &Json) -> Result<Capability, String> {
    Capability::parse(req.str_or("cap", ""))
}

fn want_subject(req: &Json) -> Subject {
    Subject::parse(req.str_or("subject", "user"))
}

fn want_undo(req: &Json) -> Undo {
    match req.get("undo") {
        Some(u) if !u.is_null() => Undo {
            note: u.str_or("note", "").to_string(),
            data: u.get("data").cloned().unwrap_or(Json::Null),
        },
        _ => Undo::none(),
    }
}

fn want_seq(req: &Json) -> Result<u64, String> {
    req.get("seq")
        .and_then(|s| s.as_u64())
        .ok_or_else(|| "expected a numeric 'seq'".to_string())
}

fn do_handle(g: &Guard, req: &Json) -> Result<Json, String> {
    match req.str_or("op", "") {
        "rule" => {
            let r = g.rule(&want_subject(req), &want_cap(req)?);
            Ok(ruling_json(&r))
        }

        "begin" => {
            let r = g.rule(&want_subject(req), &want_cap(req)?);
            let intent = req.str_or("intent", "");
            let undo = want_undo(req);
            let pending = match req.get("confirmed_by").and_then(|v| v.as_str()) {
                Some(who) => g.begin_confirmed(&r, who, intent, undo)?,
                None => g.begin(&r, intent, undo)?,
            };
            // Deliberately leaked into the protocol as a number rather than a
            // handle: the caller may crash between begin and end, and a seq it
            // can rediscover from `unfinished` is recoverable where an opaque
            // in-memory handle is not.
            Ok(json_obj([
                ("ok", true.into()),
                ("seq", pending.seq().into()),
                ("risk", r.verdict.risk.as_str().into()),
            ]))
        }

        "end" => {
            let seq = want_seq(req)?;
            let outcome = match req.str_or("outcome", "ok") {
                "ok" => Outcome::Ok,
                "failed" => Outcome::Failed,
                "refused" => Outcome::Refused,
                other => return Err(format!("unknown outcome '{}'", other)),
            };
            g.journal().end(seq, outcome, req.str_or("detail", ""))?;
            Ok(json_obj([("ok", true.into()), ("seq", seq.into())]))
        }

        "refuse" => {
            let r = g.rule(&want_subject(req), &want_cap(req)?);
            let seq = g.refuse(&r, req.str_or("intent", ""))?;
            Ok(json_obj([("ok", true.into()), ("seq", seq.into())]))
        }

        "history" => {
            let n = req.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
            Ok(json_obj([
                ("ok", true.into()),
                (
                    "records",
                    Json::Arr(g.history(n)?.iter().map(|r| r.to_json()).collect()),
                ),
            ]))
        }

        "unfinished" => Ok(json_obj([
            ("ok", true.into()),
            (
                "records",
                Json::Arr(g.unfinished()?.iter().map(|r| r.to_json()).collect()),
            ),
        ])),

        "undoable" => Ok(json_obj([
            ("ok", true.into()),
            (
                "record",
                g.undoable()?.map(|r| r.to_json()).unwrap_or(Json::Null),
            ),
        ])),

        "take_undo" => {
            let seq = want_seq(req)?;
            let u = g.take_undo(seq)?;
            Ok(json_obj([
                ("ok", true.into()),
                ("seq", seq.into()),
                ("note", u.note.into()),
                ("data", u.data),
            ]))
        }

        "reverted" => {
            let seq = want_seq(req)?;
            let by = req.get("by").and_then(|v| v.as_u64()).unwrap_or(0);
            g.reverted(seq, by)?;
            Ok(json_obj([("ok", true.into()), ("seq", seq.into())]))
        }

        "grades" => Ok(json_obj([
            ("ok", true.into()),
            (
                "grades",
                Json::Arr(
                    g.grades()
                        .known()
                        .into_iter()
                        .map(|(n, r)| {
                            json_obj([("capability", n.into()), ("risk", r.as_str().into())])
                        })
                        .collect(),
                ),
            ),
        ])),

        "absolutes" => Ok(json_obj([
            ("ok", true.into()),
            (
                "absolutes",
                Json::Arr(
                    g.policy()
                        .absolutes()
                        .iter()
                        .map(|r| {
                            json_obj([
                                ("subject", r.subject.clone().into()),
                                ("capability", r.capability.to_string().into()),
                                ("reason", r.decision.reason().into()),
                                ("cite", r.cite().into()),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])),

        "ping" => Ok(json_obj([("ok", true.into()), ("version", VERSION.into())])),

        "" => Err("expected an 'op'".to_string()),
        other => Err(format!("unknown op '{}'", other)),
    }
}
