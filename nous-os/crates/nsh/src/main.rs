//! `nsh` — the NOUS shell.
//!
//! A shell where natural language and commands share one prompt. The rule for
//! which is which is deliberately not clever: anything starting with `!` is
//! handed to `/bin/sh`, anything starting with `:` is an nsh command, and
//! everything else is an intent for the system to resolve.
//!
//! It works with no model installed. The deterministic resolver handles the
//! shapes people actually type, and only escalates when it genuinely cannot
//! tell — which is the difference between a shell and a chat window.

use nous_core::ipc::Client;
use nous_core::json::{json_obj, Json};
use nous_core::proto::method;
use std::io::{BufRead, Write};

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

fn c(code: &str) -> &str {
    if std::env::var("NO_COLOR").is_ok() {
        ""
    } else {
        code
    }
}

fn banner(client: &mut Client) {
    let status = client
        .call(method::SYS_STATUS, Json::obj())
        .unwrap_or_else(|_| Json::obj());
    let has_model = status
        .path("models.has_model")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!(
        "{}NOUS{} {}  {}·{}  {}",
        c(BOLD),
        c(RESET),
        status.str_or("version", nous_core::NOUS_VERSION),
        c(DIM),
        c(RESET),
        if has_model {
            "model reachable"
        } else {
            "no model — resolving locally"
        }
    );
    println!(
        "{}say what you want · !cmd runs a shell command · :help for more{}\n",
        c(DIM),
        c(RESET)
    );
}

fn help() {
    println!(
        "  {}anything else{}   an intent — the system resolves it and shows the plan
  {}!<command>{}      run it through /bin/sh
  {}:undo{}           reverse the last change
  {}:ledger [n]{}     what the system has done
  {}:status{}         how the machine is
  {}:find <words>{}   search your files
  {}:dry <intent>{}   resolve and preview without changing anything
  {}:help{}           this
  {}:quit{}           leave",
        c(CYAN),
        c(RESET),
        c(CYAN),
        c(RESET),
        c(CYAN),
        c(RESET),
        c(CYAN),
        c(RESET),
        c(CYAN),
        c(RESET),
        c(CYAN),
        c(RESET),
        c(CYAN),
        c(RESET),
        c(CYAN),
        c(RESET),
        c(CYAN),
        c(RESET)
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) == Some("--version") {
        println!("nsh {}", nous_core::NOUS_VERSION);
        return;
    }

    let mut client = match Client::connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("nsh: {}", e);
            std::process::exit(1);
        }
    };

    // Non-interactive: `nsh "tidy my downloads"` resolves and runs one intent.
    if !args.is_empty() {
        let text = args.join(" ");
        std::process::exit(match handle_intent(&mut client, &text, false, true) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{}error{}: {}", c(RED), c(RESET), e);
                1
            }
        });
    }

    banner(&mut client);
    let stdin = std::io::stdin();
    loop {
        print!("{}❯{} ", c(CYAN), c(RESET));
        let _ = std::io::stdout().flush();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("nsh: {}", e);
                break;
            }
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let result = if let Some(cmd) = line.strip_prefix('!') {
            shell_out(cmd)
        } else if let Some(cmd) = line.strip_prefix(':') {
            match builtin(&mut client, cmd) {
                Ok(true) => break,
                Ok(false) => Ok(()),
                Err(e) => Err(e),
            }
        } else {
            handle_intent(&mut client, line, true, false)
        };

        if let Err(e) = result {
            eprintln!("{}error{}: {}", c(RED), c(RESET), e);
        }
    }
    println!();
}

/// Returns Ok(true) when the shell should exit.
fn builtin(client: &mut Client, cmd: &str) -> Result<bool, String> {
    let (word, rest) = cmd.split_once(' ').unwrap_or((cmd, ""));
    match word {
        "quit" | "q" | "exit" => return Ok(true),
        "help" | "h" => help(),
        "undo" => {
            let out = client.call(method::JOURNAL_REVERT, Json::obj())?;
            if out.str_or("status", "") == "completed" {
                println!("{}undone{}", c(GREEN), c(RESET));
            } else {
                println!("{}", out.str_or("message", "nothing to undo"));
            }
        }
        "ledger" | "journal" => {
            let n: u64 = rest.trim().parse().unwrap_or(15);
            let out = client.call(method::JOURNAL_TAIL, json_obj([("limit", n.into())]))?;
            for r in out.arr_or_empty("records") {
                let outcome = r.str_or("outcome", "");
                let tint = match outcome {
                    "executed" | "confirmed" => c(GREEN),
                    "refused" | "failed" => c(RED),
                    _ => c(DIM),
                };
                println!(
                    "  {}{:>4}{}  {}{:<9}{}  {}",
                    c(DIM),
                    r.get("seq").and_then(|v| v.as_u64()).unwrap_or(0),
                    c(RESET),
                    tint,
                    outcome,
                    c(RESET),
                    r.str_or("detail", r.str_or("capability", ""))
                );
            }
        }
        "status" => {
            let s = client.call(method::SYS_STATUS, Json::obj())?;
            let m = s.get("metrics").cloned().unwrap_or_else(Json::obj);
            println!(
                "  load {:.2}   memory {:.0}%   disk {:.0}%   journal {} entries",
                m.f64_or("load1", 0.0),
                m.f64_or("mem_used_pct", 0.0),
                m.f64_or("disk_used_pct", 0.0),
                s.get("journal_entries")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
        }
        "find" => {
            let out = client.call(
                method::FS_SEARCH,
                json_obj([("query", rest.into()), ("limit", 15u64.into())]),
            )?;
            let results = out.arr_or_empty("results");
            if results.is_empty() {
                println!("  nothing matched");
            }
            for r in results {
                println!(
                    "  {}  {}{}{}",
                    r.str_or("name", ""),
                    c(DIM),
                    r.str_or("path", ""),
                    c(RESET)
                );
            }
        }
        "dry" => handle_intent(client, rest, true, false).map(|_| ())?,
        other => return Err(format!("no such command ':{}' — try :help", other)),
    }
    Ok(false)
}

fn shell_out(cmd: &str) -> Result<(), String> {
    // Deliberately a direct exec rather than a `shell.exec` capability: `!` is
    // the user explicitly stepping outside the governed path, and dressing that
    // up as policed would be a lie. Everything the system does on its own
    // initiative still goes through the broker.
    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .status()
        .map_err(|e| format!("cannot run: {}", e))?;
    if !status.success() {
        if let Some(code) = status.code() {
            println!("{}exit {}{}", c(DIM), code, c(RESET));
        }
    }
    Ok(())
}

fn handle_intent(
    client: &mut Client,
    text: &str,
    interactive: bool,
    auto_approve: bool,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let preflight = client.call(method::INTENT_PLAN, json_obj([("text", text.into())]))?;

    if let Some(clarify) = preflight.get("clarification").and_then(|v| v.as_str()) {
        println!("{}", clarify);
        return Ok(());
    }

    let steps = preflight.arr_or_empty("steps");
    if steps.is_empty() {
        println!("I could not work out what to do with that.");
        return Ok(());
    }

    let origin = preflight.str_or("origin", "");
    let how = if let Some(name) = origin.strip_prefix("addressed:") {
        format!("  addressed to {}", name)
    } else if origin.starts_with("model") {
        format!("  resolved by {}", origin)
    } else {
        "  understood locally".to_string()
    };
    println!("{}{}{}", c(DIM), how, c(RESET));
    for s in &steps {
        let decision = s.str_or("decision", "");
        let tint = match decision {
            "allow" => c(GREEN),
            "confirm" => c(YELLOW),
            "deny" => c(RED),
            _ => c(DIM),
        };
        println!(
            "  {}{:>7}{}  {}",
            tint,
            decision,
            c(RESET),
            s.str_or("summary", "")
        );
    }

    if preflight.bool_or("blocked", false) {
        return Err("policy refuses part of this".to_string());
    }

    let needs = preflight.bool_or("needs_approval", false);
    let approved = if needs && !auto_approve {
        if !interactive {
            println!("This needs approval; run it from an interactive shell.");
            return Ok(());
        }
        ask("  run this?")
    } else {
        true
    };
    if !approved {
        println!("  {}nothing was done{}", c(DIM), c(RESET));
        return Ok(());
    }

    let out = client.call(
        method::INTENT_SUBMIT,
        json_obj([
            ("text", text.into()),
            ("plan", preflight.get("plan").cloned().unwrap_or(Json::Null)),
            ("approved", true.into()),
        ]),
    )?;

    let mut proposal: Option<Json> = None;
    for r in out.arr_or_empty("results") {
        let state = r.str_or("state", "");
        if state == "ok" {
            println!("  {}✓{} {}", c(GREEN), c(RESET), r.str_or("detail", ""));
        } else {
            println!(
                "  {}×{} {} {}",
                c(RED),
                c(RESET),
                state,
                r.str_or("detail", "")
            );
        }
        let value = r.get("value").unwrap_or(&Json::Null);
        render_value(value);
        if is_proposal(value) {
            proposal = Some(value.clone());
        }
    }
    let message = out.str_or("message", "");
    if !message.is_empty() {
        println!("  {}{}{}", c(DIM), message, c(RESET));
    }

    // A curator proposal is read-only by itself; this is the step that closes
    // the loop, because seeing a plan with no way to say "go ahead" is not
    // actually useful. The graphical shell has an Apply button for the same
    // reason -- this is that button, in the terminal.
    if let Some(prop) = proposal {
        apply_proposal(client, &prop, interactive)?;
    }
    Ok(())
}

/// Does this look like a curator proposal -- a list of concrete moves, not
/// just the findings that led to them?
fn is_proposal(v: &Json) -> bool {
    let steps = v.get("steps").and_then(|s| s.as_arr());
    matches!(steps, Some(s) if !s.is_empty())
        && v.get("bytes").is_some()
        && steps
            .unwrap()
            .iter()
            .all(|s| s.get("capability").is_some() && s.get("summary").is_some())
}

fn apply_proposal(client: &mut Client, proposal: &Json, interactive: bool) -> Result<(), String> {
    let steps = proposal.arr_or_empty("steps");
    let n = steps.len();
    let summary = proposal.str_or("summary", "");

    if !interactive {
        println!(
            "  {}{} move(s) proposed ({}). Run nsh interactively, or `nousctl storage`,{}",
            c(DIM),
            n,
            summary,
            c(RESET)
        );
        println!(
            "  {}to review and apply this from a terminal.{}",
            c(DIM),
            c(RESET)
        );
        return Ok(());
    }

    if !ask(&format!("  Apply {} move(s) ({})?", n, summary)) {
        println!("  {}nothing was moved{}", c(DIM), c(RESET));
        return Ok(());
    }

    let out = client.call(
        "cap.invoke",
        json_obj([
            ("capability", "curate.apply".into()),
            ("args", json_obj([("steps", Json::Arr(steps))])),
            ("approved", true.into()),
            ("why", "apply a tidy-up proposal".into()),
        ]),
    )?;
    for r in out.arr_or_empty("results") {
        let state = r.str_or("state", "");
        let tint = if state == "ok" { c(GREEN) } else { c(RED) };
        let mark = if state == "ok" { "✓" } else { "×" };
        println!("  {}{}{} {}", tint, mark, c(RESET), r.str_or("detail", ""));
    }
    Ok(())
}

/// Render the shapes the executors return, so a listing looks like a listing
/// rather than a wall of JSON.
fn render_value(v: &Json) {
    // An assistant's answer is prose; print it as prose, and say where it came
    // from, because "did that leave my machine?" is part of the answer.
    if let Some(answer) = v.get("answer").and_then(|a| a.as_str()) {
        let where_ = if v.bool_or("local", false) {
            "on this machine".to_string()
        } else {
            format!("via {}", v.str_or("backend", "?"))
        };
        println!(
            "      {}{} — {}{}",
            c(DIM),
            v.str_or("assistant", "assistant"),
            where_,
            c(RESET)
        );
        for line in answer.lines() {
            println!("      {}", line);
        }
        return;
    }
    if let Some(entries) = v.get("entries").and_then(|e| e.as_arr()) {
        for e in entries.iter().take(40) {
            println!(
                "      {}{:<34}{} {}",
                if e.bool_or("is_dir", false) {
                    c(CYAN)
                } else {
                    ""
                },
                e.str_or("name", ""),
                c(RESET),
                if e.bool_or("is_dir", false) {
                    String::new()
                } else {
                    human(e.get("size").and_then(|s| s.as_u64()).unwrap_or(0))
                }
            );
        }
        return;
    }
    if is_proposal(v) {
        let steps = v.arr_or_empty("steps");
        println!(
            "      {} move(s) proposed ({})",
            steps.len(),
            v.str_or("summary", "")
        );
        for s in steps.iter().take(8) {
            println!("        {} {}", c(DIM), s.str_or("summary", ""));
        }
        if steps.len() > 8 {
            println!(
                "        {}… and {} more{}",
                c(DIM),
                steps.len() - 8,
                c(RESET)
            );
        }
        return;
    }
    if let Some(findings) = v.get("findings").and_then(|f| f.as_arr()) {
        for f in findings {
            println!(
                "      {} {}({}){}",
                f.str_or("title", ""),
                c(DIM),
                f.str_or("detail", ""),
                c(RESET)
            );
        }
        return;
    }
    if let Some(procs) = v.get("processes").and_then(|p| p.as_arr()) {
        for p in procs.iter().take(12) {
            println!(
                "      {:>7}  {:<22} {}",
                p.get("pid").and_then(|x| x.as_u64()).unwrap_or(0),
                p.str_or("name", ""),
                human(p.get("rss_kb").and_then(|x| x.as_u64()).unwrap_or(0) * 1024)
            );
        }
        return;
    }
    if v.get("load1").is_some() {
        println!(
            "      load {:.2}   memory {:.0}%   disk {:.0}% used",
            v.f64_or("load1", 0.0),
            v.f64_or("mem_used_pct", 0.0),
            v.f64_or("disk_used_pct", 0.0)
        );
    }
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

fn ask(prompt: &str) -> bool {
    print!("{} [y/N] ", prompt);
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn move_step(summary: &str) -> Json {
        json_obj([
            ("capability", "fs.move:/x".into()),
            ("summary", summary.into()),
        ])
    }

    #[test]
    fn recognises_a_real_curator_proposal() {
        let v = json_obj([
            (
                "steps",
                Json::Arr(vec![move_step("move a.txt"), move_step("move b.txt")]),
            ),
            ("bytes", 1024i64.into()),
            ("summary", "1.0 KB".into()),
            ("findings", Json::Arr(vec![])),
        ]);
        assert!(is_proposal(&v));
    }

    #[test]
    fn does_not_mistake_other_shapes_for_a_proposal() {
        // No steps at all.
        assert!(!is_proposal(&json_obj([("findings", Json::Arr(vec![]))])));
        // Steps present but not move-shaped (e.g. an echoed GLYPH plan).
        let not_moves = json_obj([
            ("steps", Json::Arr(vec![Json::obj()])),
            ("bytes", 10i64.into()),
        ]);
        assert!(!is_proposal(&not_moves));
        // Steps and the right shape, but no "bytes" -- not a curator proposal.
        let no_bytes = json_obj([("steps", Json::Arr(vec![move_step("move a.txt")]))]);
        assert!(!is_proposal(&no_bytes));
        // An empty steps array is not a proposal worth acting on.
        assert!(!is_proposal(&json_obj([
            ("steps", Json::Arr(vec![])),
            ("bytes", 0i64.into())
        ])));
    }

    #[test]
    fn formats_sizes_readably() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(2048), "2.0 KB");
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn renders_a_directory_listing_without_panicking_on_odd_shapes() {
        let v = json_obj([(
            "entries",
            Json::Arr(vec![
                json_obj([("name", "Documents".into()), ("is_dir", true.into())]),
                // A malformed entry must not take the shell down.
                Json::Null,
            ]),
        )]);
        render_value(&v);
    }

    #[test]
    fn render_value_ignores_shapes_it_does_not_know() {
        render_value(&Json::Null);
        render_value(&json_obj([("something", "unexpected".into())]));
    }
}
