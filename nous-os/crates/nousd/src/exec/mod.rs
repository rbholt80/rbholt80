//! Executors: the only place in NOUS OS where capabilities become effects.
//!
//! Everything above this layer reasons about *intent*. Everything below it is
//! the machine. The broker has already decided the step is permitted by the
//! time an executor sees it; an executor's job is to do the thing and to hand
//! back an [`Undo`] that puts the machine back.

pub mod curate;
pub mod fsops;
pub mod media;
pub mod sysops;

use nous_core::journal::Undo;
use nous_core::{Capability, Config, Json, Journal, Step};
use std::path::PathBuf;

/// Everything an executor is allowed to reach.
pub struct ExecCtx<'a> {
    pub cfg: &'a Config,
    pub journal: &'a Journal,
    /// When set, executors compute and describe their effect but do not apply
    /// it. Every mutating executor must honour this.
    pub dry_run: bool,
    /// Resolved home directory, so executors never re-derive it inconsistently.
    pub home: PathBuf,
    /// Root of mutable daemon state (trash, caches, projects).
    pub state: PathBuf,
}

impl<'a> ExecCtx<'a> {
    pub fn new(cfg: &'a Config, journal: &'a Journal, dry_run: bool) -> ExecCtx<'a> {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"));
        ExecCtx { cfg, journal, dry_run, home, state: nous_core::ipc::state_dir() }
    }

    /// Same, but with the home and state roots pinned. Used by tests, and by the
    /// installer when it operates on a target filesystem that is not `/`.
    pub fn rooted(
        cfg: &'a Config,
        journal: &'a Journal,
        dry_run: bool,
        home: PathBuf,
        state: PathBuf,
    ) -> ExecCtx<'a> {
        ExecCtx { cfg, journal, dry_run, home, state }
    }

    /// Where reversible deletes go.
    pub fn trash_dir(&self) -> PathBuf {
        self.state.join("trash")
    }
}

/// The result of running one step.
#[derive(Debug)]
pub struct Effect {
    /// Payload returned to the caller.
    pub result: Json,
    /// How to reverse this, recorded in the journal.
    pub undo: Undo,
    /// One line for the audit trail and the UI.
    pub detail: String,
}

impl Effect {
    pub fn read_only(result: Json, detail: impl Into<String>) -> Effect {
        Effect { result, undo: Undo::None, detail: detail.into() }
    }

    pub fn with_undo(result: Json, undo: Undo, detail: impl Into<String>) -> Effect {
        Effect { result, undo, detail: detail.into() }
    }
}

/// Dispatch a step to the executor that owns its capability domain.
pub fn execute(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let cap = Capability::parse(&step.capability)?;
    match cap.domain.as_str() {
        "fs" => fsops::execute(&cap, step, ctx),
        "media" => media::execute(&cap, step, ctx),
        "curate" => curate::execute(&cap, step, ctx),
        "sys" | "proc" | "svc" | "pkg" | "shell" | "net" => sysops::execute(&cap, step, ctx),
        other => Err(format!("no executor for capability domain '{}'", other)),
    }
}

/// Reverse a previously journalled action.
pub fn revert(undo: &Undo, ctx: &ExecCtx) -> Result<String, String> {
    match undo {
        Undo::None => Err("this action recorded nothing to undo".to_string()),
        Undo::RestoreFile { path, backup, existed } => {
            let target = PathBuf::from(path);
            if *existed {
                let src = backup.as_ref().ok_or("the snapshot for this action is missing")?;
                std::fs::copy(src, &target)
                    .map_err(|e| format!("cannot restore {}: {}", path, e))?;
                Ok(format!("restored {}", path))
            } else {
                if target.exists() {
                    std::fs::remove_file(&target)
                        .map_err(|e| format!("cannot remove {}: {}", path, e))?;
                }
                Ok(format!("removed {}", path))
            }
        }
        Undo::MovePath { from, to } => {
            fsops::move_path(&PathBuf::from(to), &PathBuf::from(from))?;
            Ok(format!("moved {} back to {}", to, from))
        }
        Undo::RemoveDir { path } => {
            let p = PathBuf::from(path);
            if p.exists() {
                // Only remove it if it is still empty: the user may have put
                // something there since, and an undo must not destroy that.
                std::fs::remove_dir(&p).map_err(|e| {
                    format!("cannot remove {} (is it still empty?): {}", path, e)
                })?;
            }
            Ok(format!("removed directory {}", path))
        }
        Undo::ServiceState { unit, was_active } => {
            let verb = if *was_active { "start" } else { "stop" };
            sysops::systemctl(&[verb, unit], ctx)?;
            Ok(format!("{}ed {}", verb, unit))
        }
        Undo::Manual { note } => Err(format!("this must be undone by hand: {}", note)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::json::json_obj;

    fn ctx_for<'a>(cfg: &'a Config, j: &'a Journal) -> ExecCtx<'a> {
        let tmp = std::env::temp_dir().join("nous-exec-state");
        ExecCtx::rooted(cfg, j, false, tmp.clone(), tmp)
    }

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("nous-exec-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn unknown_domains_are_rejected_not_guessed() {
        let dir = scratch("dispatch");
        let cfg = Config::with_defaults();
        let j = Journal::open(&dir).unwrap();
        let step = Step::new("s", "quantum.entangle:/x", "?", "", Json::obj());
        let err = execute(&step, &ctx_for(&cfg, &j)).unwrap_err();
        assert!(err.contains("no executor"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_undo_removes_a_file_that_did_not_exist_before() {
        let dir = scratch("undo-create");
        let cfg = Config::with_defaults();
        let j = Journal::open(&dir).unwrap();
        let f = dir.join("created.txt");
        std::fs::write(&f, b"new").unwrap();
        let undo = Undo::RestoreFile {
            path: f.to_string_lossy().to_string(),
            backup: None,
            existed: false,
        };
        revert(&undo, &ctx_for(&cfg, &j)).unwrap();
        assert!(!f.exists(), "undoing a create should remove the file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_undo_puts_previous_contents_back() {
        let dir = scratch("undo-write");
        let cfg = Config::with_defaults();
        let j = Journal::open(&dir).unwrap();
        let f = dir.join("edited.txt");
        std::fs::write(&f, b"original").unwrap();
        let backup = j.snapshot(&f).unwrap().unwrap();
        std::fs::write(&f, b"clobbered").unwrap();

        let undo = Undo::RestoreFile {
            path: f.to_string_lossy().to_string(),
            backup: Some(backup),
            existed: true,
        };
        revert(&undo, &ctx_for(&cfg, &j)).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "original");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undoing_a_mkdir_refuses_to_destroy_new_contents() {
        let dir = scratch("undo-mkdir");
        let cfg = Config::with_defaults();
        let j = Journal::open(&dir).unwrap();
        let made = dir.join("made");
        std::fs::create_dir(&made).unwrap();
        std::fs::write(made.join("something-the-user-added"), b"!").unwrap();

        let undo = Undo::RemoveDir { path: made.to_string_lossy().to_string() };
        assert!(revert(&undo, &ctx_for(&cfg, &j)).is_err());
        assert!(made.exists(), "the user's file must survive the undo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manual_undo_explains_itself_rather_than_pretending() {
        let dir = scratch("undo-manual");
        let cfg = Config::with_defaults();
        let j = Journal::open(&dir).unwrap();
        let undo = Undo::Manual { note: "re-pair the bluetooth device".into() };
        let err = revert(&undo, &ctx_for(&cfg, &j)).unwrap_err();
        assert!(err.contains("re-pair"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn effect_helpers_carry_their_intent() {
        let e = Effect::read_only(json_obj([("n", 3i64.into())]), "listed 3 entries");
        assert!(e.undo.is_none());
        assert_eq!(e.detail, "listed 3 entries");
    }
}
