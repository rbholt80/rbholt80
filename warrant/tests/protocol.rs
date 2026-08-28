//! The binary, driven the way another language drives it.
//!
//! The library tests cover the decisions. These cover the thing that is
//! actually shipped to a Python or Kotlin host: a process, two pipes, and one
//! JSON object per line. A protocol that is only ever exercised through its own
//! Rust types is not a protocol.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use warrant::json::{parse, Json};

const POLICY: &str = "\
never   *        fs.read:/**/.ssh/**   # keys stay on the disk
deny    agent:*  pkg.install           # a person installs software
confirm agent:*  fs.delete:~/**        # say it out loud
allow   agent:*  fs.write:~/**
allow   agent:*  fs.read:~/**
";

const GRADES: &str = "\
read     fs.read
write    fs.write
elevated fs.delete pkg.install
";

struct Harness {
    dir: PathBuf,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Harness {
    fn start(tag: &str) -> Harness {
        let dir = std::env::temp_dir().join(format!(
            "warrant-proto-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("policy.warrant"), POLICY).unwrap();
        std::fs::write(dir.join("grades.warrant"), GRADES).unwrap();

        let mut child = Command::new(bin())
            .args(["--dir", dir.to_str().unwrap()])
            .args(["--home", "/home/robert"])
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn warrant");

        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Harness {
            dir,
            child,
            stdin,
            stdout,
        }
    }

    fn ask(&mut self, line: &str) -> Json {
        writeln!(self.stdin, "{}", line).unwrap();
        self.stdin.flush().unwrap();
        let mut reply = String::new();
        let n = self.stdout.read_line(&mut reply).unwrap();
        assert!(n > 0, "the guard gave no answer to: {}", line);
        parse(&reply).unwrap_or_else(|e| panic!("reply was not JSON ({}): {}", e, reply))
    }

    fn ok(&mut self, line: &str) -> Json {
        let v = self.ask(line);
        assert!(
            v.bool_or("ok", false),
            "expected success for {} — got {}",
            line,
            v
        );
        v
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn bin() -> PathBuf {
    // The test binary lives in target/<profile>/deps; the CLI is two up.
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("warrant")
}

#[test]
fn an_allowed_action_runs_end_to_end() {
    let mut h = Harness::start("allowed");

    let r = h.ok(r#"{"op":"rule","subject":"agent:claude","cap":"fs.write:/home/robert/a.md"}"#);
    assert_eq!(r.str_or("decision", ""), "allow");
    assert_eq!(r.str_or("risk", ""), "write");

    let b = h.ok(
        r#"{"op":"begin","subject":"agent:claude","cap":"fs.write:/home/robert/a.md", "intent":"save the draft", "undo":{"note":"restore a.md","data":{"backup":"/var/b/1"}}}"#,
    );
    let seq = b.get("seq").unwrap().as_u64().unwrap();

    h.ok(&format!(
        r#"{{"op":"end","seq":{},"outcome":"ok","detail":"wrote 412 bytes"}}"#,
        seq
    ));

    let hist = h.ok(r#"{"op":"history","limit":10}"#);
    let recs = hist.arr_or_empty("records");
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].str_or("outcome", ""), "ok");
    assert_eq!(recs[0].str_or("intent", ""), "save the draft");
    assert_eq!(recs[0].str_or("detail", ""), "wrote 412 bytes");
}

#[test]
fn a_denied_action_cannot_be_begun_over_the_wire() {
    // The decision must be enforced by the guard, not by the caller
    // remembering to check first. A host that skips `rule` and goes straight
    // to `begin` still gets refused.
    let mut h = Harness::start("denied");
    let v = h.ask(r#"{"op":"begin","subject":"agent:claude","cap":"pkg.install:htop"}"#);
    assert!(!v.bool_or("ok", true), "{}", v);
    assert!(v.str_or("error", "").contains("not authorised"), "{}", v);
}

#[test]
fn a_never_cannot_be_confirmed_past_over_the_wire() {
    // The one rule a caller must not be able to talk its way around, tested
    // through the interface a caller actually has.
    let mut h = Harness::start("never");
    let v = h.ask(
        r#"{"op":"begin","subject":"agent:claude","cap":"fs.read:/home/robert/.ssh/id_rsa", "confirmed_by":"robert"}"#,
    );
    assert!(
        !v.bool_or("ok", true),
        "a never line was confirmed past: {}",
        v
    );
    assert!(
        v.str_or("error", "").contains("cannot be confirmed"),
        "{}",
        v
    );
}

#[test]
fn a_confirm_needs_someone_named() {
    let mut h = Harness::start("confirm");
    let cap = "fs.delete:/home/robert/old.txt";

    let r = h.ok(&format!(
        r#"{{"op":"rule","subject":"agent:claude","cap":"{}"}}"#,
        cap
    ));
    assert_eq!(r.str_or("decision", ""), "confirm");
    assert!(r.str_or("prompt", "").contains("say it out loud"), "{}", r);

    let v = h.ask(&format!(
        r#"{{"op":"begin","subject":"agent:claude","cap":"{}"}}"#,
        cap
    ));
    assert!(
        !v.bool_or("ok", true),
        "confirm was treated as allow: {}",
        v
    );

    let ok = h.ok(&format!(
        r#"{{"op":"begin","subject":"agent:claude","cap":"{}","confirmed_by":"robert"}}"#,
        cap
    ));
    let seq = ok.get("seq").unwrap().as_u64().unwrap();
    h.ok(&format!(r#"{{"op":"end","seq":{},"outcome":"ok"}}"#, seq));

    let recs = h
        .ok(r#"{"op":"history","limit":10}"#)
        .arr_or_empty("records");
    assert_eq!(recs[0].str_or("decision", ""), "confirmed by robert");
}

#[test]
fn an_undo_survives_the_guard_being_restarted() {
    // The reversal is on disk, not in the process. Kill the guard between
    // begin and undo — which is what a crash is — and it is still there.
    let mut h = Harness::start("restart");
    let b = h.ok(
        r#"{"op":"begin","subject":"agent:claude","cap":"fs.write:/home/robert/a.md", "undo":{"note":"restore a.md","data":{"backup":"/var/b/9"}}}"#,
    );
    let seq = b.get("seq").unwrap().as_u64().unwrap();
    h.ok(&format!(r#"{{"op":"end","seq":{},"outcome":"ok"}}"#, seq));

    let dir = h.dir.clone();
    let mut child = Command::new(bin())
        .args(["--dir", dir.to_str().unwrap()])
        .args(["--home", "/home/robert"])
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut second = Harness {
        dir: h.dir.clone(),
        stdin: child.stdin.take().unwrap(),
        stdout: BufReader::new(child.stdout.take().unwrap()),
        child,
    };

    let u = second.ok(&format!(r#"{{"op":"take_undo","seq":{}}}"#, seq));
    assert_eq!(u.str_or("note", ""), "restore a.md");
    assert_eq!(u.get("data").unwrap().str_or("backup", ""), "/var/b/9");

    // Both harnesses point at the same dir; let the first one clean it up.
    std::mem::forget(std::mem::replace(
        &mut second.dir,
        PathBuf::from("/nonexistent"),
    ));
}

#[test]
fn a_bad_request_gets_an_answer_not_a_dropped_connection() {
    // A host is blocked reading a reply. Every failure has to come back as a
    // reply, or the caller hangs instead of handling an error.
    let mut h = Harness::start("badreq");
    for line in [
        "{ this is not json",
        r#"{"op":"nonsense"}"#,
        r#"{"op":"rule","subject":"user","cap":"notacapability"}"#,
        r#"{"op":"end","seq":"not a number"}"#,
        r#"{}"#,
    ] {
        let v = h.ask(line);
        assert!(!v.bool_or("ok", true), "{} should have failed", line);
        assert!(
            !v.str_or("error", "").is_empty(),
            "no reason given for {}",
            line
        );
    }
    // Still alive and still answering afterwards.
    assert!(h.ok(r#"{"op":"ping"}"#).bool_or("ok", false));
}

#[test]
fn check_answers_with_its_exit_code() {
    // So a shell script or a tool hook can branch without parsing anything.
    let h = Harness::start("exitcode");
    let code = |subject: &str, cap: &str| -> i32 {
        Command::new(bin())
            .args(["--dir", h.dir.to_str().unwrap()])
            .args(["--home", "/home/robert"])
            .args(["check", subject, cap])
            .stdout(Stdio::null())
            .status()
            .unwrap()
            .code()
            .unwrap()
    };
    assert_eq!(code("agent:claude", "fs.write:/home/robert/a.md"), 0);
    assert_eq!(code("agent:claude", "fs.delete:/home/robert/a.md"), 10);
    assert_eq!(code("agent:claude", "pkg.install:htop"), 20);
    assert_eq!(code("agent:claude", "fs.read:/home/robert/.ssh/id_rsa"), 30);
    assert_eq!(code("agent:claude", "not a capability"), 64);
}

#[test]
fn init_writes_files_that_parse() {
    // The starting policy has to actually load. A broken example is worse than
    // none, because it is the first thing anybody runs.
    let dir = std::env::temp_dir().join(format!("warrant-init-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let out = Command::new(bin())
        .args(["init", dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());

    let status = Command::new(bin())
        .args(["--dir", dir.to_str().unwrap()])
        .args(["check", "agent:claude", "fs.read:/home/robert/a.md"])
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(
        status.code().unwrap() < 64,
        "the starting policy did not load"
    );

    // And it does not clobber a policy that is already there.
    std::fs::write(dir.join("policy.warrant"), "allow user fs.read\n").unwrap();
    Command::new(bin())
        .args(["init", dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("policy.warrant")).unwrap(),
        "allow user fs.read\n"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_journal_is_greppable_by_a_person() {
    // The promise that `jq` and `grep` work. If this breaks, the audit log
    // stops being something anybody actually reads.
    let mut h = Harness::start("greppable");
    let b = h.ok(
        r#"{"op":"begin","subject":"agent:claude","cap":"fs.write:/home/robert/a.md", "intent":"save","undo":{"note":"restore","data":{}}}"#,
    );
    let seq = b.get("seq").unwrap().as_u64().unwrap();
    h.ok(&format!(r#"{{"op":"end","seq":{},"outcome":"ok"}}"#, seq));

    let text = std::fs::read_to_string(h.dir.join("journal.ndjson")).unwrap();
    assert_eq!(text.lines().count(), 2);
    for line in text.lines() {
        assert!(
            parse(line).is_ok(),
            "not one JSON object per line: {}",
            line
        );
    }
    assert!(text.contains("agent:claude"));
}
