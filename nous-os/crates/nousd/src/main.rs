//! `nousd` — the NOUS OS system daemon.
//!
//! One process owns the AI subsystems: the capability broker, the policy
//! engine, the journal, the model router, the intent resolver, the file index
//! and the sensorium. Everything else on the machine — the shell, the graphical
//! desktop, the control tool, third-party agents — is a client of this socket.
//!
//! Running it as a daemon rather than a library in each application is what
//! makes the guarantees hold: there is exactly one policy engine, exactly one
//! journal, and no way to reach an executor except through the broker.

mod assist;
mod broker;
mod bus;
mod exec;
mod httpc;
mod hwprofile;
mod index;
mod maintenance;
mod resolve;
mod router;
mod sensorium;
mod webui;

use broker::{Broker, RunOptions};
use bus::Bus;
use nous_core::ipc::{self, read_frame, write_frame};
use nous_core::journal::now_secs;
use nous_core::json::{json_obj, Json};
use nous_core::proto::{errcode, method, Frame, Request, Response};
use nous_core::{log_error, log_info, log_warn};
use nous_core::{Config, Journal, Plan, Policy, Step, Subject};
use resolve::{Context, Resolver};
use router::Router;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const MODULE: &str = "nousd";

pub struct Daemon {
    pub cfg: Config,
    pub bus: Arc<Bus>,
    pub broker: Arc<Broker>,
    pub router: Arc<Router>,
    pub resolver: Arc<Resolver>,
    pub started: u64,
    intents: AtomicU64,
}

impl Daemon {
    fn next_intent_id(&self) -> String {
        format!("i{}", self.intents.fetch_add(1, Ordering::Relaxed))
    }

    /// Answer one request.
    pub fn handle(&self, req: &Request) -> Response {
        match req.method.as_str() {
            method::PING => Response::ok(
                &req.id,
                json_obj([
                    ("pong", true.into()),
                    ("version", nous_core::NOUS_VERSION.into()),
                ]),
            ),

            method::SYS_STATUS => Response::ok(&req.id, self.status()),
            method::MODEL_STATUS => Response::ok(&req.id, self.router.status()),

            method::INTENT_PLAN => {
                let text = req.param_str("text").unwrap_or("").trim().to_string();
                if text.is_empty() {
                    return Response::err(&req.id, errcode::BAD_REQUEST, "no text to resolve");
                }
                let context = self.context_of(req);
                let plan = self.resolver.resolve_with_context(
                    &self.next_intent_id(),
                    &text,
                    &self.router,
                    &context,
                );
                let mut out = self.broker.preflight(&plan, &Subject::User);
                out.set("plan", plan.to_json());
                if let Some(c) = &plan.clarification {
                    out.set("clarification", Json::Str(c.clone()));
                }
                Response::ok(&req.id, out)
            }

            method::INTENT_SUBMIT => {
                let text = req.param_str("text").unwrap_or("").trim().to_string();
                if text.is_empty() {
                    return Response::err(&req.id, errcode::BAD_REQUEST, "no text to resolve");
                }
                // A plan supplied by the client is one it has already been
                // shown and approved; resolving again could produce a different
                // plan from the one the human said yes to.
                let plan = match req.params.get("plan") {
                    Some(p) if !p.is_null() => Plan::from_json(p),
                    _ => self.resolver.resolve_with_context(
                        &self.next_intent_id(),
                        &text,
                        &self.router,
                        &self.context_of(req),
                    ),
                };
                if let Some(c) = &plan.clarification {
                    return Response::err_with(
                        &req.id,
                        errcode::BAD_REQUEST,
                        c.clone(),
                        plan.to_json(),
                    );
                }
                let opts = RunOptions {
                    subject: Subject::User,
                    dry_run: req.param_bool("dry_run", false),
                    approved: req.param_bool("approved", false),
                };
                Response::ok(&req.id, self.broker.run(&plan, &opts))
            }

            method::INTENT_CONFIRM => {
                let plan = match req.params.get("plan") {
                    Some(p) if !p.is_null() => Plan::from_json(p),
                    _ => {
                        return Response::err(
                            &req.id,
                            errcode::BAD_REQUEST,
                            "confirming needs the plan that was approved",
                        )
                    }
                };
                let opts = RunOptions {
                    subject: Subject::User,
                    dry_run: false,
                    approved: req.param_bool("approve", true),
                };
                if !opts.approved {
                    return Response::ok(
                        &req.id,
                        json_obj([
                            ("status", "declined".into()),
                            ("intent_id", plan.intent_id.clone().into()),
                        ]),
                    );
                }
                Response::ok(&req.id, self.broker.run(&plan, &opts))
            }

            method::CAP_CHECK => {
                let cap = req.param_str("capability").unwrap_or("");
                match nous_core::Capability::parse(cap) {
                    Ok(c) => {
                        let v = self.broker.policy.evaluate(&Subject::User, &c);
                        Response::ok(
                            &req.id,
                            json_obj([
                                ("capability", c.to_string().into()),
                                ("decision", v.decision.kind().into()),
                                ("risk", v.risk.to_string().into()),
                                ("explain", v.explain().into()),
                            ]),
                        )
                    }
                    Err(e) => Response::err(&req.id, errcode::BAD_REQUEST, e),
                }
            }

            // The general-purpose entry point: exercise one capability
            // directly. The graphical shell drives everything through this, so
            // the UI has no privileged path of its own.
            method::AGENT_INVOKE | "cap.invoke" => {
                let cap = req.param_str("capability").unwrap_or("").to_string();
                if cap.is_empty() {
                    return Response::err(&req.id, errcode::BAD_REQUEST, "no capability given");
                }
                let args = req.params.get("args").cloned().unwrap_or_else(Json::obj);
                let handler = cap.split('.').next().unwrap_or("fs").to_string();
                let step = Step::new("s1", &cap, &handler, &cap, args);
                let plan = Plan {
                    intent_id: self.next_intent_id(),
                    utterance: req.param_str("why").unwrap_or(&cap).to_string(),
                    steps: vec![step],
                    origin: "direct".to_string(),
                    confidence: 1.0,
                    clarification: None,
                };
                let opts = RunOptions {
                    subject: Subject::User,
                    dry_run: req.param_bool("dry_run", false),
                    approved: req.param_bool("approved", false),
                };
                Response::ok(&req.id, self.broker.run(&plan, &opts))
            }

            method::JOURNAL_TAIL => {
                let n = req.param_u64("limit", 40) as usize;
                match self.broker.journal.tail(n) {
                    Ok(records) => Response::ok(
                        &req.id,
                        json_obj([
                            (
                                "records",
                                Json::Arr(records.iter().map(|r| r.to_json()).collect()),
                            ),
                            ("count", records.len().into()),
                        ]),
                    ),
                    Err(e) => Response::err(&req.id, errcode::INTERNAL, e),
                }
            }

            method::JOURNAL_REVERT => {
                let mut args = Json::obj();
                if let Some(seq) = req.params.get("seq").and_then(|v| v.as_u64()) {
                    args.set("seq", seq.into());
                }
                let step = Step::new("s1", "journal.revert", "journal", "undo", args);
                let plan = Plan {
                    intent_id: self.next_intent_id(),
                    utterance: "undo".to_string(),
                    steps: vec![step],
                    origin: "direct".to_string(),
                    confidence: 1.0,
                    clarification: None,
                };
                Response::ok(
                    &req.id,
                    self.broker.run(
                        &plan,
                        &RunOptions {
                            approved: true,
                            ..Default::default()
                        },
                    ),
                )
            }

            method::FS_SEARCH => {
                let idx = index::Index::load();
                Response::ok(
                    &req.id,
                    idx.search_json(
                        req.param_str("query").unwrap_or(""),
                        req.params.get("kind").and_then(|v| v.as_str()),
                        req.param_u64("limit", 40) as usize,
                    ),
                )
            }

            method::FS_INDEX => {
                let roots = self.cfg.paths("index.roots");
                let idx = index::Index::build(&roots, &self.cfg);
                let n = idx.docs.len();
                match idx.save() {
                    Ok(()) => Response::ok(&req.id, json_obj([("indexed", n.into())])),
                    Err(e) => Response::err(&req.id, errcode::INTERNAL, e),
                }
            }

            method::MODEL_COMPLETE => {
                let prompt = req.param_str("prompt").unwrap_or("");
                if prompt.is_empty() {
                    return Response::err(&req.id, errcode::BAD_REQUEST, "no prompt");
                }
                let mut c = router::Completion::new(
                    req.param_str("system")
                        .unwrap_or("You are a helpful assistant."),
                    prompt,
                );
                if req.param_str("tier") == Some("small") {
                    c.tier = router::Tier::Small;
                }
                match self.router.complete(&c) {
                    Ok(s) => Response::ok(
                        &req.id,
                        json_obj([
                            ("text", s.text.into()),
                            ("backend", s.backend.into()),
                            ("model", s.model.into()),
                        ]),
                    ),
                    Err(e) => Response::err(&req.id, errcode::BACKEND_UNAVAILABLE, e),
                }
            }

            // What is being kept, and a way to reclaim it. Read-only unless
            // `apply` is set, so you see the cost before agreeing to lose it.
            "sys.maintenance" => {
                let state = maintenance::state_root();
                let apply = req.param_bool("apply", false);
                let report = maintenance::run(&self.broker.journal, &self.cfg, &state, !apply);
                let mut out = report.to_json();
                out.set("applied", apply.into());
                out.set("usage", maintenance::usage(&self.broker.journal, &state));
                Response::ok(&req.id, out)
            }

            method::SYS_SHUTDOWN => Response::ok(&req.id, json_obj([("stopping", true.into())])),

            other => Response::err(
                &req.id,
                errcode::UNKNOWN_METHOD,
                format!("nousd has no method '{}'", other),
            ),
        }
    }

    /// The context a client attached to its request, filled in from the live
    /// session where the client could not know it.
    fn context_of(&self, req: &Request) -> Context {
        let mut ctx = match req.params.get("context") {
            Some(c) if !c.is_null() => Context::from_json(c),
            _ => Context::default(),
        };
        // The overlay captures the focused window before it appears, because by
        // the time it is up, the focused window is the overlay. A client that
        // did not do that gets whatever is focused now, which is better than
        // nothing for a terminal or a script.
        if ctx.focus.is_none() {
            if let Some(title) = exec::desktop::focus_context()
                .get("focused_window")
                .and_then(|v| v.as_str())
            {
                ctx.focus = Some(title.to_string());
            }
        }
        ctx
    }

    pub fn status(&self) -> Json {
        let hw = hwprofile::detect();
        let journal_len = self.broker.journal.read_all().map(|r| r.len()).unwrap_or(0);
        json_obj([
            ("name", nous_core::NOUS_NAME.into()),
            ("version", nous_core::NOUS_VERSION.into()),
            (
                "uptime_secs",
                (now_secs().saturating_sub(self.started)).into(),
            ),
            ("system", exec::sysops::sys_info()),
            ("metrics", exec::sysops::sys_metrics()),
            ("hardware", hw.to_json()),
            ("models", self.router.status()),
            ("policy_rules", self.broker.policy.rules.len().into()),
            ("desktop", exec::desktop::session_info()),
            (
                "storage",
                maintenance::usage(&self.broker.journal, &maintenance::state_root()),
            ),
            ("journal_entries", journal_len.into()),
            (
                "bus",
                json_obj([
                    ("subscribers", self.bus.subscriber_count().into()),
                    ("published", self.bus.published_count().into()),
                    ("dropped", self.bus.dropped_count().into()),
                ]),
            ),
        ])
    }
}

/// Load policy from every configuration directory, then the builtin defaults
/// underneath. First match wins, so the user's own rules override the site's,
/// and the site's override the defaults.
fn load_policy_from(dirs: &[PathBuf]) -> Policy {
    let mut policy = Policy::empty();
    for dir in dirs {
        policy.extend(read_policy_dir(dir));
    }
    policy.extend(Policy::builtin());
    policy
}

/// Read one `policy.d` directory. Unreadable and malformed files are reported
/// and skipped; a typo must never silently fail open.
fn read_policy_dir(dir: &std::path::Path) -> Policy {
    let mut policy = Policy::empty();
    let policy_d = dir.join("policy.d");
    if let Ok(entries) = std::fs::read_dir(&policy_d) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("conf"))
            .collect();
        files.sort();
        for f in files {
            match std::fs::read_to_string(&f) {
                Ok(text) => {
                    let name = f.file_name().and_then(|s| s.to_str()).unwrap_or("policy");
                    match Policy::parse(&text, name) {
                        Ok(p) => {
                            log_info!(MODULE, "loaded policy {}", f.display());
                            policy.extend(p);
                        }
                        // A malformed policy file must not silently fail open.
                        Err(e) => log_error!(MODULE, "ignoring {}: {}", f.display(), e),
                    }
                }
                Err(e) => log_warn!(MODULE, "cannot read {}: {}", f.display(), e),
            }
        }
    }
    policy
}

fn serve_connection(daemon: Arc<Daemon>, stream: UnixStream) {
    let mut reader = match stream.try_clone() {
        Ok(s) => BufReader::new(s),
        Err(e) => {
            log_warn!(MODULE, "cannot clone connection: {}", e);
            return;
        }
    };
    let mut writer = stream;
    let mut subscription: Option<u64> = None;

    loop {
        let frame = match read_frame(&mut reader) {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                let _ = write_frame(&mut writer, &ipc::transport_error("", &e).to_json());
                break;
            }
        };
        let req = match Frame::parse(&frame) {
            Ok(Frame::Req(r)) => r,
            Ok(_) => continue,
            Err(e) => {
                let _ = write_frame(&mut writer, &ipc::transport_error("", &e).to_json());
                continue;
            }
        };

        if req.method == method::SUBSCRIBE {
            if let Some(old) = subscription.take() {
                daemon.bus.unsubscribe(old);
            }
            let topics = req.params.str_list("topics");
            let (id, rx) = daemon.bus.subscribe(topics);
            subscription = Some(id);
            let _ = write_frame(
                &mut writer,
                &Response::ok(&req.id, json_obj([("subscribed", true.into())])).to_json(),
            );

            // Events are pushed from a second thread so the connection can keep
            // answering requests while a long flow narrates itself.
            let mut event_writer = match writer.try_clone() {
                Ok(w) => w,
                Err(_) => continue,
            };
            let bus = daemon.bus.clone();
            std::thread::spawn(move || {
                while let Ok(event) = rx.recv() {
                    if write_frame(&mut event_writer, &event.to_json()).is_err() {
                        break;
                    }
                    bus.ack(id, 1);
                }
            });
            continue;
        }

        let response = daemon.handle(&req);
        if write_frame(&mut writer, &response.to_json()).is_err() {
            break;
        }
    }

    if let Some(id) = subscription {
        daemon.bus.unsubscribe(id);
    }
}

fn print_usage() {
    println!(
        "nousd {} — the NOUS OS system daemon

USAGE:
    nousd [OPTIONS]

OPTIONS:
    --socket PATH     listen here instead of the default
    --config DIR      read nous.conf and policy.d from here
    --state DIR       keep the journal, index and trash here
    --http PORT       also serve the graphical shell on this port (0 disables)
    --check           validate configuration and policy, then exit
    --version         print the version and exit
    -h, --help        print this message

The socket defaults to $XDG_RUNTIME_DIR/nous.sock, then /run/nous/nous.sock.",
        nous_core::NOUS_VERSION
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut socket_override: Option<PathBuf> = None;
    let mut http_port: Option<u16> = None;
    let mut check_only = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--version" | "-V" => {
                println!("nousd {}", nous_core::NOUS_VERSION);
                return;
            }
            "-h" | "--help" => {
                print_usage();
                return;
            }
            "--check" => check_only = true,
            "--socket" => {
                i += 1;
                socket_override = args.get(i).map(PathBuf::from);
            }
            "--config" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    std::env::set_var("NOUS_CONFIG_DIR", v);
                }
            }
            "--state" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    std::env::set_var("NOUS_STATE_DIR", v);
                }
            }
            "--http" => {
                i += 1;
                http_port = args.get(i).and_then(|v| v.parse().ok());
            }
            other => {
                eprintln!("nousd: unknown option '{}'\n", other);
                print_usage();
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let cfg = Config::load();
    nous_core::log::set_level(nous_core::log::Level::parse(
        cfg.str_or("log.level", "info"),
    ));

    let policy = load_policy_from(&ipc::config_dirs());
    let journal_dir = cfg.journal_dir();
    let journal = match Journal::open(&journal_dir) {
        Ok(j) => j,
        Err(e) => {
            eprintln!(
                "nousd: cannot open journal at {}: {}",
                journal_dir.display(),
                e
            );
            std::process::exit(1);
        }
    };

    if check_only {
        println!("configuration: ok ({} keys)", cfg.keys_under("").len());
        println!("policy:        ok ({} rules)", policy.rules.len());
        println!("journal:       ok ({})", journal_dir.display());
        println!("hardware:\n{}", hwprofile::detect().report());
        return;
    }

    let bus = Arc::new(Bus::new());
    let router = Arc::new(Router::from_config(&cfg));
    let resolver = Arc::new(Resolver::from_config(&cfg));
    let broker = Arc::new(Broker::new(
        cfg.clone(),
        policy,
        journal,
        bus.clone(),
        router.clone(),
    ));
    let daemon = Arc::new(Daemon {
        cfg: cfg.clone(),
        bus: bus.clone(),
        broker,
        router,
        resolver,
        started: now_secs(),
        intents: AtomicU64::new(1),
    });

    let socket = socket_override.unwrap_or_else(ipc::socket_path);
    let listener = match ipc::bind(&socket) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("nousd: {}", e);
            std::process::exit(1);
        }
    };

    let hw = hwprofile::detect();
    log_info!(
        MODULE,
        "NOUS {} listening on {}",
        nous_core::NOUS_VERSION,
        socket.display()
    );
    log_info!(
        MODULE,
        "hardware profile: {} — {}",
        hw.profile.as_str(),
        hw.profile.explain()
    );
    if !daemon.router.has_model() {
        log_info!(
            MODULE,
            "no model backend reachable; the deterministic resolver will handle intents"
        );
    }

    // The sensorium samples the machine in the background.
    let sensor = sensorium::Sensorium::new(bus.clone(), cfg.clone());
    std::thread::spawn(move || sensor.run());

    // And the system tidies up after itself, on a much slower timer. Running
    // once at startup matters: a machine that is only on for an hour a day
    // would otherwise never reach the interval.
    {
        let broker = daemon.broker.clone();
        let cfg = cfg.clone();
        let bus = bus.clone();
        std::thread::spawn(move || {
            let every = Duration::from_secs(cfg.u64_or("retain.interval_secs", 21_600).max(60));
            loop {
                let state = maintenance::state_root();
                let report = maintenance::run(&broker.journal, &cfg, &state, false);
                if !report.is_empty() {
                    log_info!(MODULE, "housekeeping: {}", report.describe());
                    bus.publish(nous_core::Event::new(
                        nous_core::proto::topic::SENSOR,
                        json_obj([("kind", "maintenance".into()), ("report", report.to_json())]),
                    ));
                }
                std::thread::sleep(every);
            }
        });
    }

    // The graphical shell is served over HTTP on loopback.
    let port = http_port.unwrap_or_else(|| cfg.u64_or("ui.port", 7666) as u16);
    if port != 0 {
        let d = daemon.clone();
        std::thread::spawn(move || {
            if let Err(e) = webui::serve(d, port) {
                log_error!(MODULE, "graphical shell unavailable: {}", e);
            }
        });
    }

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let d = daemon.clone();
                std::thread::spawn(move || serve_connection(d, stream));
            }
            Err(e) => log_warn!(MODULE, "connection failed: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon(tag: &str) -> (PathBuf, Arc<Daemon>) {
        let root = std::env::temp_dir().join(format!("nous-daemon-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let cfg = Config::with_defaults();
        let bus = Arc::new(Bus::new());
        let journal = Journal::open(&root.join("journal")).unwrap();
        let router = Arc::new(Router::from_config(&cfg));
        let broker = Arc::new(Broker::new(
            cfg.clone(),
            Policy::builtin(),
            journal,
            bus.clone(),
            router.clone(),
        ));
        let d = Arc::new(Daemon {
            cfg: cfg.clone(),
            bus,
            broker,
            router,
            resolver: Arc::new(Resolver::from_config(&cfg)),
            started: now_secs(),
            intents: AtomicU64::new(1),
        });
        (root, d)
    }

    fn call(d: &Daemon, m: &str, params: Json) -> Response {
        d.handle(&Request::new("1", m, params))
    }

    #[test]
    fn answers_ping() {
        let (root, d) = daemon("ping");
        let r = call(&d, method::PING, Json::obj());
        assert!(r.is_ok());
        assert!(r.into_result().unwrap().bool_or("pong", false));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_methods_get_a_stable_error_code() {
        let (root, d) = daemon("unknown");
        match call(&d, "does.not.exist", Json::obj()) {
            Response::Err { code, .. } => assert_eq!(code, errcode::UNKNOWN_METHOD),
            Response::Ok { .. } => panic!("expected an error"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn status_describes_the_running_system() {
        let (root, d) = daemon("status");
        let s = call(&d, method::SYS_STATUS, Json::obj())
            .into_result()
            .unwrap();
        assert_eq!(s.str_or("name", ""), "NOUS");
        assert!(s.get("hardware").is_some());
        assert!(s.get("models").is_some());
        assert!(s.get("policy_rules").unwrap().as_u64().unwrap() > 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn planning_shows_the_steps_without_running_them() {
        let (root, d) = daemon("plan");
        let out = call(
            &d,
            method::INTENT_PLAN,
            json_obj([("text", "show my downloads".into())]),
        )
        .into_result()
        .unwrap();
        let steps = out.arr_or_empty("steps");
        assert_eq!(steps.len(), 1);
        assert!(steps[0].str_or("capability", "").starts_with("fs.list:"));
        assert_eq!(steps[0].str_or("decision", ""), "allow");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_intent_is_rejected_rather_than_guessed_at() {
        let (root, d) = daemon("empty");
        assert!(!call(&d, method::INTENT_PLAN, json_obj([("text", "   ".into())])).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_destructive_intent_reports_that_it_needs_approval() {
        let (root, d) = daemon("approval");
        let out = call(
            &d,
            "cap.invoke",
            json_obj([
                ("capability", "fs.delete:/home/nobody/x".into()),
                ("args", json_obj([("path", "/home/nobody/x".into())])),
            ]),
        )
        .into_result()
        .unwrap();
        assert_eq!(out.str_or("status", ""), "needs_approval");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cap_check_explains_a_decision_without_acting() {
        let (root, d) = daemon("capcheck");
        let out = call(
            &d,
            method::CAP_CHECK,
            json_obj([("capability", "fs.write:/boot/x".into())]),
        )
        .into_result()
        .unwrap();
        assert_eq!(out.str_or("decision", ""), "deny");
        assert!(
            out.str_or("explain", "").contains("protected"),
            "{}",
            out.str_or("explain", "")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn invoking_a_read_capability_returns_its_result() {
        let (root, d) = daemon("invoke");
        let out = call(
            &d,
            "cap.invoke",
            json_obj([("capability", "sys.metrics".into())]),
        )
        .into_result()
        .unwrap();
        assert_eq!(out.str_or("status", ""), "completed");
        let results = out.arr_or_empty("results");
        assert!(results[0].path("value.cpus").is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn site_policy_overrides_a_builtin_default() {
        let dir = std::env::temp_dir().join(format!("nous-policyd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("policy.d")).unwrap();
        std::fs::write(
            dir.join("policy.d/10-local.conf"),
            "deny user fs.read:/home/**   # this machine is locked down\n",
        )
        .unwrap();

        let policy = load_policy_from(std::slice::from_ref(&dir));
        let cap = nous_core::Capability::parse("fs.read:/home/joey/notes.md").unwrap();
        let v = policy.evaluate(&Subject::User, &cap);
        assert!(
            matches!(v.decision, nous_core::Decision::Deny(_)),
            "site policy loads above the builtin and must win: {}",
            v.explain()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_policy_overrides_system_policy() {
        // The case that matters for an install over an existing distribution:
        // rules in ~/.config/nous must win over anything in /etc/nous.
        let base = std::env::temp_dir().join(format!("nous-twodir-{}", std::process::id()));
        let (user, system) = (base.join("user"), base.join("system"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(user.join("policy.d")).unwrap();
        std::fs::create_dir_all(system.join("policy.d")).unwrap();

        std::fs::write(
            system.join("policy.d/50-site.conf"),
            "deny  user  shell.exec  # site says no\n",
        )
        .unwrap();
        std::fs::write(
            user.join("policy.d/10-mine.conf"),
            "allow user  shell.exec  # my machine\n",
        )
        .unwrap();

        let policy = load_policy_from(&[user.clone(), system.clone()]);
        let cap = nous_core::Capability::parse("shell.exec:ls").unwrap();
        assert!(
            policy.evaluate(&Subject::User, &cap).decision.is_allow(),
            "the user's own rule should be reached first"
        );

        // Reverse the search order and the site rule wins instead.
        let reversed = load_policy_from(&[system, user]);
        assert!(matches!(
            reversed.evaluate(&Subject::User, &cap).decision,
            nous_core::Decision::Deny(_)
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_malformed_policy_file_does_not_fail_open() {
        let dir = std::env::temp_dir().join(format!("nous-badpolicy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("policy.d")).unwrap();
        std::fs::write(
            dir.join("policy.d/10-bad.conf"),
            "permit everyone everything\n",
        )
        .unwrap();

        let policy = load_policy_from(std::slice::from_ref(&dir));
        // The bad file is skipped, but the builtin floor is still in place.
        let cap = nous_core::Capability::parse("fs.write:/boot/x").unwrap();
        assert!(matches!(
            policy.evaluate(&Subject::User, &cap).decision,
            nous_core::Decision::Deny(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
