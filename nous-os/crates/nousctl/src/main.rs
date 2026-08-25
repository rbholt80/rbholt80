//! `nousctl` — control and inspect a running NOUS system.
//!
//! Everything here is available in the graphical shell too. It exists because
//! an OS you can only administer through its desktop is an OS you cannot fix
//! when the desktop is what broke.

use nous_core::glyph;
use nous_core::ipc::Client;
use nous_core::journal::format_ts;
use nous_core::json::{json_obj, Json};
use nous_core::proto::method;
use nous_core::{Policy, Subject};

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";

fn colour(code: &str) -> &str {
    // Respect NO_COLOR, and drop colour when piped.
    if std::env::var("NO_COLOR").is_ok() {
        ""
    } else {
        code
    }
}

fn usage() -> i32 {
    println!(
        "{}nousctl{} {} — control the NOUS system

{}USAGE{}
    nousctl <command> [arguments]

{}COMMANDS{}
    status                  what the daemon is doing
    doctor                  inspect this machine and its model profile
    ledger [n]              the last n journalled actions (default 20)
    undo [seq]              reverse the last action, or a specific one
    ask <words...>          resolve an intent and show the plan
    run <words...>          resolve an intent and run it
    check <capability>      ask policy about one capability
    policy                  print the effective policy
    glyph check <file>      parse and check a GLYPH program
    glyph run <file>        run a GLYPH program's first flow
    models                  model backends and their state
    key set <provider>      store an API key (read from stdin)
    index                   rebuild the file index
    storage                 what NOUS is keeping, and what it can reclaim
    find <words...>         search indexed files
    version",
        colour(BOLD),
        colour(RESET),
        nous_core::NOUS_VERSION,
        colour(BOLD),
        colour(RESET),
        colour(BOLD),
        colour(RESET)
    );
    2
}

fn connect() -> Result<Client, String> {
    Client::connect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(|s| s.as_str()) {
        None | Some("-h") | Some("--help") | Some("help") => usage(),
        Some("version") | Some("--version") | Some("-V") => {
            println!("nousctl {}", nous_core::NOUS_VERSION);
            0
        }
        Some("status") => run(cmd_status()),
        Some("doctor") => run(cmd_doctor()),
        Some("ledger") | Some("journal") => run(cmd_ledger(
            args.get(1).and_then(|n| n.parse().ok()).unwrap_or(20),
        )),
        Some("undo") => run(cmd_undo(args.get(1).and_then(|n| n.parse().ok()))),
        Some("ask") => run(cmd_intent(&args[1..].join(" "), false)),
        Some("run") => run(cmd_intent(&args[1..].join(" "), true)),
        Some("check") => run(cmd_check(args.get(1).map(|s| s.as_str()).unwrap_or(""))),
        Some("policy") => run(cmd_policy()),
        Some("glyph") => run(cmd_glyph(&args[1..])),
        Some("models") => run(cmd_models()),
        Some("key") => run(cmd_key(&args[1..])),
        Some("index") => run(cmd_index()),
        Some("storage") | Some("maintenance") => run(cmd_storage(
            args.get(1).map(|s| s.as_str()) == Some("--clean"),
        )),
        Some("find") => run(cmd_find(&args[1..].join(" "))),
        Some(other) => {
            eprintln!("nousctl: unknown command '{}'", other);
            usage()
        }
    };
    std::process::exit(code);
}

fn run(result: Result<(), String>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}error{}: {}", colour(RED), colour(RESET), e);
            1
        }
    }
}

fn cmd_status() -> Result<(), String> {
    let mut c = connect()?;
    let s = c.call(method::SYS_STATUS, Json::obj())?;
    let sys = s.get("system").cloned().unwrap_or_else(Json::obj);
    let m = s.get("metrics").cloned().unwrap_or_else(Json::obj);

    println!(
        "{}NOUS {}{}  on {}",
        colour(BOLD),
        s.str_or("version", "?"),
        colour(RESET),
        sys.str_or("hostname", "?")
    );
    println!(
        "  {}up{}          {} minutes",
        colour(DIM),
        colour(RESET),
        s.get("uptime_secs").and_then(|v| v.as_u64()).unwrap_or(0) / 60
    );
    println!(
        "  {}system{}      {}",
        colour(DIM),
        colour(RESET),
        sys.str_or("distro", "?")
    );
    println!(
        "  {}profile{}     {}",
        colour(DIM),
        colour(RESET),
        s.path("hardware.profile")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
    );
    println!(
        "  {}policy{}      {} rules",
        colour(DIM),
        colour(RESET),
        s.get("policy_rules").and_then(|v| v.as_u64()).unwrap_or(0)
    );
    println!(
        "  {}journal{}     {} entries",
        colour(DIM),
        colour(RESET),
        s.get("journal_entries")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    );
    println!(
        "  {}load{}        {:.2}   {}memory{} {:.0}%   {}disk{} {:.0}%",
        colour(DIM),
        colour(RESET),
        m.f64_or("load1", 0.0),
        colour(DIM),
        colour(RESET),
        m.f64_or("mem_used_pct", 0.0),
        colour(DIM),
        colour(RESET),
        m.f64_or("disk_used_pct", 0.0)
    );
    let has_model = s
        .path("models.has_model")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!(
        "  {}models{}      {}",
        colour(DIM),
        colour(RESET),
        if has_model {
            "reachable"
        } else {
            "none — using the deterministic resolver"
        }
    );
    Ok(())
}

fn cmd_doctor() -> Result<(), String> {
    // Works without the daemon: this is the command you run before installing.
    match connect() {
        Ok(mut c) => {
            let s = c.call(method::SYS_STATUS, Json::obj())?;
            let hw = s.get("hardware").cloned().unwrap_or_else(Json::obj);
            println!("{}This machine{}", colour(BOLD), colour(RESET));
            println!(
                "  CPU     {} ({} cores)",
                hw.str_or("cpu_model", "?"),
                hw.get("cpus").and_then(|v| v.as_u64()).unwrap_or(0)
            );
            println!(
                "  Memory  {:.1} GB",
                hw.get("ram_mb").and_then(|v| v.as_u64()).unwrap_or(0) as f64 / 1024.0
            );
            println!(
                "  Disk    {:.1} GB free",
                hw.get("disk_free_mb").and_then(|v| v.as_u64()).unwrap_or(0) as f64 / 1024.0
            );
            for g in hw.arr_or_empty("gpus") {
                println!(
                    "  GPU     {} ({})",
                    g.str_or("name", "?"),
                    g.str_or("vendor", "?")
                );
            }
            println!(
                "\n  {}Profile{} {}",
                colour(BOLD),
                colour(RESET),
                hw.str_or("profile", "?")
            );
            println!("          {}", hw.str_or("explain", ""));
            if let Some(model) = hw.get("local_model").and_then(|v| v.as_str()) {
                println!("          Local model: {}", model);
            }
            for n in hw.arr_or_empty("notes") {
                println!(
                    "\n  {}Note{}    {}",
                    colour(YELLOW),
                    colour(RESET),
                    n.as_str().unwrap_or("")
                );
            }
            Ok(())
        }
        Err(_) => {
            println!("The daemon is not running. Start it with:  nousd");
            println!("Run `nousd --check` to inspect this machine without starting it.");
            Ok(())
        }
    }
}

fn cmd_ledger(n: u64) -> Result<(), String> {
    let mut c = connect()?;
    let out = c.call(method::JOURNAL_TAIL, json_obj([("limit", n.into())]))?;
    let records = out.arr_or_empty("records");
    if records.is_empty() {
        println!("Nothing has happened yet.");
        return Ok(());
    }
    for r in records {
        let outcome = r.str_or("outcome", "");
        let mark = match outcome {
            "executed" | "confirmed" => format!("{}·{}", colour(GREEN), colour(RESET)),
            "refused" => format!("{}×{}", colour(RED), colour(RESET)),
            "failed" => format!("{}!{}", colour(RED), colour(RESET)),
            _ => format!("{}◦{}", colour(DIM), colour(RESET)),
        };
        let undone = r.get("undone_by").and_then(|v| v.as_u64());
        println!(
            "{} {}{:>4}{}  {}  {}{}",
            mark,
            colour(DIM),
            r.get("seq").and_then(|v| v.as_u64()).unwrap_or(0),
            colour(RESET),
            format_ts(r.get("ts").and_then(|v| v.as_u64()).unwrap_or(0)),
            r.str_or("detail", r.str_or("capability", "?")),
            match undone {
                Some(by) => format!("  {}(undone by {}){}", colour(DIM), by, colour(RESET)),
                None => String::new(),
            }
        );
    }
    println!(
        "\n{}undo the last change with:{} nousctl undo",
        colour(DIM),
        colour(RESET)
    );
    Ok(())
}

fn cmd_undo(seq: Option<u64>) -> Result<(), String> {
    let mut c = connect()?;
    let mut params = Json::obj();
    if let Some(s) = seq {
        params.set("seq", s.into());
    }
    let out = c.call(method::JOURNAL_REVERT, params)?;
    let status = out.str_or("status", "");
    if status == "completed" {
        println!(
            "{}undone{} — {}",
            colour(GREEN),
            colour(RESET),
            out.arr_or_empty("results")
                .first()
                .map(|r| r.str_or("detail", "").to_string())
                .unwrap_or_default()
        );
        Ok(())
    } else {
        Err(out.str_or("message", "could not undo").to_string())
    }
}

fn print_plan(preflight: &Json) {
    println!(
        "{}{}{}",
        colour(BOLD),
        preflight.str_or("utterance", ""),
        colour(RESET)
    );
    let origin = preflight.str_or("origin", "");
    println!(
        "  {}{}{}\n",
        colour(DIM),
        if origin.starts_with("model") {
            format!("resolved by {}", origin)
        } else {
            "understood locally".to_string()
        },
        colour(RESET)
    );
    for s in preflight.arr_or_empty("steps") {
        let decision = s.str_or("decision", "");
        let tint = match decision {
            "allow" => colour(GREEN),
            "confirm" => colour(YELLOW),
            "deny" => colour(RED),
            _ => colour(DIM),
        };
        println!(
            "  {}{:>7}{}  {}",
            tint,
            decision,
            colour(RESET),
            s.str_or("summary", "")
        );
        println!(
            "           {}{}{}",
            colour(DIM),
            s.str_or("capability", ""),
            colour(RESET)
        );
        let reason = s.str_or("reason", "");
        if !reason.is_empty() && decision != "allow" {
            println!("           {}{}{}", colour(DIM), reason, colour(RESET));
        }
    }
}

fn cmd_intent(text: &str, execute: bool) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("say what you want, e.g. nousctl ask \"tidy my downloads\"".to_string());
    }
    let mut c = connect()?;
    let preflight = c.call(method::INTENT_PLAN, json_obj([("text", text.into())]))?;

    if let Some(clarify) = preflight.get("clarification").and_then(|v| v.as_str()) {
        println!("{}", clarify);
        return Ok(());
    }
    print_plan(&preflight);

    if !execute {
        return Ok(());
    }
    if preflight.bool_or("blocked", false) {
        return Err("policy refuses part of this plan".to_string());
    }
    if preflight.bool_or("needs_approval", false) && !confirm("\nRun this?") {
        println!("Nothing was done.");
        return Ok(());
    }

    let out = c.call(
        method::INTENT_SUBMIT,
        json_obj([
            ("text", text.into()),
            ("plan", preflight.get("plan").cloned().unwrap_or(Json::Null)),
            ("approved", true.into()),
        ]),
    )?;
    println!();
    for r in out.arr_or_empty("results") {
        let state = r.str_or("state", "");
        let tint = if state == "ok" {
            colour(GREEN)
        } else {
            colour(RED)
        };
        println!(
            "  {}{}{}  {}",
            tint,
            if state == "ok" { "✓" } else { "×" },
            colour(RESET),
            r.str_or("detail", r.str_or("summary", ""))
        );
    }
    let message = out.str_or("message", "");
    if !message.is_empty() {
        println!("\n{}", message);
    }
    Ok(())
}

fn confirm(prompt: &str) -> bool {
    use std::io::Write;
    print!("{} [y/N] ", prompt);
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn cmd_check(cap: &str) -> Result<(), String> {
    if cap.is_empty() {
        return Err("give a capability, e.g. nousctl check fs.delete:/home/me/x".to_string());
    }
    let mut c = connect()?;
    let out = c.call(method::CAP_CHECK, json_obj([("capability", cap.into())]))?;
    let decision = out.str_or("decision", "");
    let tint = match decision {
        "allow" => colour(GREEN),
        "confirm" => colour(YELLOW),
        _ => colour(RED),
    };
    println!(
        "{}  {}{}{}  ({} risk)",
        out.str_or("capability", cap),
        tint,
        decision,
        colour(RESET),
        out.str_or("risk", "?")
    );
    println!(
        "{}{}{}",
        colour(DIM),
        out.str_or("explain", ""),
        colour(RESET)
    );
    Ok(())
}

fn cmd_policy() -> Result<(), String> {
    // Printed from the builtin so it works with the daemon stopped; the daemon
    // reports its own rule count in `status`.
    let policy = Policy::builtin();
    println!(
        "{}effective policy{} ({} builtin rules, first match wins)\n",
        colour(BOLD),
        colour(RESET),
        policy.rules.len()
    );
    for r in &policy.rules {
        println!(
            "  {:<8} {:<16} {}",
            r.decision.kind(),
            r.subject,
            r.capability
        );
    }
    println!(
        "\n{}site rules in /etc/nous/policy.d/*.conf load above these{}",
        colour(DIM),
        colour(RESET)
    );
    Ok(())
}

fn cmd_glyph(args: &[String]) -> Result<(), String> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    let path = args.get(1).ok_or("give a .glyph file")?;
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path, e))?;

    match sub {
        "check" => {
            let manifests = glyph::lint(&src, glyph::current_platform())?;
            let policy = Policy::builtin();
            let mut failed = false;
            for m in &manifests {
                println!(
                    "{}flow {}{}  — {}",
                    colour(BOLD),
                    m.flow,
                    colour(RESET),
                    m.blast_radius()
                );
                if !m.description.is_empty() {
                    println!("  {}{}{}", colour(DIM), m.description, colour(RESET));
                }
                for d in &m.diagnostics {
                    let tint = if d.severity == glyph::Severity::Error {
                        colour(RED)
                    } else {
                        colour(YELLOW)
                    };
                    println!("  {}{}{}", tint, d.render(&m.flow), colour(RESET));
                    if d.severity == glyph::Severity::Error {
                        failed = true;
                    }
                }
                println!("\n  {}what it may do{}", colour(BOLD), colour(RESET));
                for (cap, decision) in m.preflight(&policy, &Subject::User) {
                    let tint = match decision.kind() {
                        "allow" => colour(GREEN),
                        "confirm" => colour(YELLOW),
                        _ => colour(RED),
                    };
                    println!(
                        "    {}{:>7}{}  {}",
                        tint,
                        decision.kind(),
                        colour(RESET),
                        cap
                    );
                }
                if m.asks > 0 || m.gates > 0 {
                    println!(
                        "\n  {}{} confirmation(s), {} gate(s){}",
                        colour(DIM),
                        m.asks,
                        m.gates,
                        colour(RESET)
                    );
                }
                println!();
            }
            if failed {
                return Err("this program does not check out".to_string());
            }
            println!("{}✓ checks out{}", colour(GREEN), colour(RESET));
            Ok(())
        }
        "run" => {
            let manifests = glyph::lint(&src, glyph::current_platform())?;
            let m = manifests.first().ok_or("no flows in this file")?;
            if !m.is_valid() {
                for d in m.errors() {
                    eprintln!("{}", d.render(&m.flow));
                }
                return Err("this program does not check out".to_string());
            }
            println!(
                "{}flow {}{} — {}",
                colour(BOLD),
                m.flow,
                colour(RESET),
                m.blast_radius()
            );
            if !confirm("Run it?") {
                println!("Nothing was done.");
                return Ok(());
            }
            let mut c = connect()?;
            let out = c.call(
                method::INTENT_SUBMIT,
                json_obj([
                    ("text", format!("glyph:{}", m.flow).into()),
                    ("approved", true.into()),
                ]),
            )?;
            println!("{}", out.to_string_pretty());
            Ok(())
        }
        _ => Err("usage: nousctl glyph check|run <file>".to_string()),
    }
}

fn cmd_models() -> Result<(), String> {
    let mut c = connect()?;
    let s = c.call(method::MODEL_STATUS, Json::obj())?;
    println!(
        "{}route{}       {}",
        colour(BOLD),
        colour(RESET),
        s.str_list("route").join(" → ")
    );
    println!(
        "{}small route{} {}",
        colour(BOLD),
        colour(RESET),
        s.str_list("route_small").join(" → ")
    );
    println!();
    for b in s.arr_or_empty("backends") {
        let up = b.bool_or("available", false);
        println!(
            "  {}{}{}  {:<28} {}",
            if up { colour(GREEN) } else { colour(DIM) },
            if up { "●" } else { "○" },
            colour(RESET),
            b.str_or("name", "?"),
            b.str_or("model", "")
        );
    }
    println!(
        "\n{}add a key with:{} nousctl key set anthropic",
        colour(DIM),
        colour(RESET)
    );
    Ok(())
}

fn cmd_key(args: &[String]) -> Result<(), String> {
    if args.first().map(|s| s.as_str()) != Some("set") {
        return Err("usage: nousctl key set <provider>   (the key is read from stdin)".to_string());
    }
    let provider = args
        .get(1)
        .ok_or("which provider? anthropic, openai, openrouter")?;
    eprintln!("Paste the key for {} and press enter:", provider);
    let mut key = String::new();
    std::io::stdin()
        .read_line(&mut key)
        .map_err(|e| format!("cannot read the key: {}", e))?;
    let key = key.trim();
    if key.is_empty() {
        return Err("no key given".to_string());
    }
    // The same code the daemon reads with, so the format cannot drift.
    nous_core::Secrets::set(provider, key)?;
    println!(
        "{}stored{} — restart nousd to pick it up",
        colour(GREEN),
        colour(RESET)
    );
    Ok(())
}

fn cmd_storage(clean: bool) -> Result<(), String> {
    let mut c = connect()?;
    let out = c.call("sys.maintenance", json_obj([("apply", clean.into())]))?;
    let usage = out.get("usage").cloned().unwrap_or_else(Json::obj);

    let row = |label: &str, bytes: u64| {
        println!(
            "  {}{:<12}{} {}",
            colour(DIM),
            label,
            colour(RESET),
            human(bytes)
        );
    };
    println!(
        "{}NOUS is keeping{} {}",
        colour(BOLD),
        colour(RESET),
        usage.str_or("total", "?")
    );
    println!(
        "  {}{}{}\n",
        colour(DIM),
        usage.str_or("path", ""),
        colour(RESET)
    );
    for (label, key) in [
        ("journal", "journal_bytes"),
        ("trash", "trash_bytes"),
        ("thumbnails", "thumbnail_bytes"),
        ("screenshots", "screenshot_bytes"),
        ("index", "index_bytes"),
    ] {
        row(label, usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0));
    }

    println!();
    let summary = out.str_or("summary", "nothing to clean up");
    if out.bool_or("applied", false) {
        println!("{}cleaned{} — {}", colour(GREEN), colour(RESET), summary);
    } else if summary == "nothing to clean up" {
        println!("{}", summary);
    } else {
        println!(
            "{}would reclaim{} — {}",
            colour(YELLOW),
            colour(RESET),
            summary
        );
        let kept = out
            .get("kept_for_undo")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if kept > 0 {
            println!(
                "{}{} snapshot(s) kept because their actions can still be undone{}",
                colour(DIM),
                kept,
                colour(RESET)
            );
        }
        println!(
            "\n{}run `nousctl storage --clean` to do it{}",
            colour(DIM),
            colour(RESET)
        );
    }
    Ok(())
}

fn human(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, U[0])
    } else {
        format!("{:.1} {}", v, U[i])
    }
}

fn cmd_index() -> Result<(), String> {
    let mut c = connect()?;
    println!("indexing…");
    let out = c.call(method::FS_INDEX, Json::obj())?;
    println!(
        "{} files indexed",
        out.get("indexed").and_then(|v| v.as_u64()).unwrap_or(0)
    );
    Ok(())
}

fn cmd_find(query: &str) -> Result<(), String> {
    if query.trim().is_empty() {
        return Err("what are you looking for?".to_string());
    }
    let mut c = connect()?;
    let out = c.call(
        method::FS_SEARCH,
        json_obj([("query", query.into()), ("limit", 20u64.into())]),
    )?;
    let results = out.arr_or_empty("results");
    if results.is_empty() {
        println!("Nothing matched. Run `nousctl index` if you have not indexed yet.");
        return Ok(());
    }
    for r in results {
        println!(
            "  {}  {}{}{}",
            r.str_or("name", ""),
            colour(DIM),
            r.str_or("path", ""),
            colour(RESET)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_strings_from_the_cli_parse() {
        assert!(nous_core::Capability::parse("fs.delete:/home/me/x").is_ok());
        assert!(nous_core::Capability::parse("garbage").is_err());
    }

    #[test]
    fn formats_sizes_readably() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KB");
    }

    #[test]
    fn colour_is_suppressed_when_asked() {
        std::env::set_var("NO_COLOR", "1");
        assert_eq!(colour(GREEN), "");
        std::env::remove_var("NO_COLOR");
    }
}
