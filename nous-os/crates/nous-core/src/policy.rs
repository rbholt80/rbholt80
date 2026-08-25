//! Policy engine.
//!
//! The broker asks one question — "may this subject exercise this capability?" —
//! and policy answers Allow, Confirm or Deny. Rules are ordered and the first
//! match wins, which makes a policy file readable top-to-bottom: narrow
//! exceptions above broad defaults.
//!
//! Policy is a plain text file so that a human can audit it without tooling:
//!
//! ```text
//! # decision   subject            capability              # reason
//! deny         *                  secret.read             # never
//! confirm      agent:fs-agent     fs.delete:/home/**       # ask before deleting
//! allow        user               fs.read:/home/**
//! ```

use crate::cap::{protected_violation, Capability, Risk};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Proceed without asking.
    Allow,
    /// Proceed only after the human says yes.
    Confirm(String),
    /// Refuse.
    Deny(String),
}

impl Decision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow)
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Confirm(_) => "confirm",
            Decision::Deny(_) => "deny",
        }
    }
    pub fn reason(&self) -> &str {
        match self {
            Decision::Allow => "",
            Decision::Confirm(r) | Decision::Deny(r) => r,
        }
    }
}

/// Who is asking. Agents are third-party code and are named; the human at the
/// console is `Subject::User`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    User,
    Agent(String),
    System,
}

impl Subject {
    pub fn label(&self) -> String {
        match self {
            Subject::User => "user".to_string(),
            Subject::System => "system".to_string(),
            Subject::Agent(id) => format!("agent:{}", id),
        }
    }

    pub fn parse(s: &str) -> Subject {
        match s {
            "user" => Subject::User,
            "system" => Subject::System,
            other => Subject::Agent(other.trim_start_matches("agent:").to_string()),
        }
    }

    fn matches(&self, pattern: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        if pattern == "agent:*" {
            return matches!(self, Subject::Agent(_));
        }
        pattern == self.label()
    }
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub decision: Decision,
    pub subject: String,
    pub capability: Capability,
    pub source: String,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Policy {
    pub rules: Vec<Rule>,
    /// Subjects may never exceed this risk without an explicit `allow` rule.
    pub agent_risk_ceiling: Risk,
}

/// The verdict, with enough provenance to explain itself to the user.
#[derive(Debug, Clone)]
pub struct Verdict {
    pub decision: Decision,
    pub matched: Option<String>,
    pub risk: Risk,
}

impl Verdict {
    pub fn explain(&self) -> String {
        let src = self
            .matched
            .clone()
            .unwrap_or_else(|| "default".to_string());
        match &self.decision {
            Decision::Allow => format!("allowed by {} (risk: {})", src, self.risk),
            Decision::Confirm(r) => format!("needs confirmation — {} [{}]", r, src),
            Decision::Deny(r) => format!("denied — {} [{}]", r, src),
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Policy::builtin()
    }
}

impl Policy {
    /// The policy the system falls back to when no file is present.
    ///
    /// It is deliberately usable: a machine with no configuration should still
    /// let you ask questions about your own files, and should still refuse to
    /// let an agent quietly install packages.
    pub fn builtin() -> Policy {
        const DEFAULTS: &str = r#"
# --- absolute refusals -------------------------------------------------------
deny     *              secret.read              # secrets never enter context
deny     agent:*        policy.amend             # agents cannot rewrite policy
deny     agent:*        user.admin               # nor manage users
deny     agent:*        sys.firmware
deny     agent:*        sys.power                # only the human powers down

# --- observation is free -----------------------------------------------------
allow    *              fs.read:~/**
allow    *              fs.read:/tmp/**
allow    *              fs.list
allow    *              fs.stat
allow    *              fs.search
allow    *              proc.list
allow    *              sys.info
allow    *              sys.metrics
allow    *              svc.status
allow    *              net.status
allow    *              pkg.query
allow    *              ctx.read
allow    *              journal.read
allow    *              fs.index
allow    *              model.infer
allow    *              media.probe
allow    *              media.search
allow    *              media.thumbnail
allow    *              curate.scan
allow    *              curate.propose
allow    *              desk.apps
allow    *              desk.windows
allow    *              desk.session_info

# --- the user's own machine, for the user ------------------------------------
allow    user           fs.write:~/**
allow    user           fs.mkdir:~/**
allow    user           fs.move:~/**
allow    user           ui.notify
allow    user           ui.render
allow    user           ctx.write
allow    user           journal.revert           # undo is never harder than the act
allow    user           desk.notify
allow    user           desk.copy
allow    user           desk.focus
allow    user           desk.open                # opening a file in its usual app
confirm  user           desk.clipboard           # the clipboard may hold a password
confirm  user           desk.screenshot          # so may whatever is on screen
confirm  user           desk.launch
confirm  user           desk.close               # a window may hold unsaved work
confirm  user           desk.setting
confirm  user           desk.session
allow    user           media.play
allow    user           media.control
allow    user           media.edit
allow    user           media.index
allow    user           media.render:~/**
confirm  user           curate.apply             # tidying always shows its plan first
allow    user           assist.ask               # you configured the key; that was the consent
allow    user           assist.list
confirm  user           fs.delete:/**            # deletion always asks
confirm  user           shell.exec               # so does running arbitrary code
confirm  user           pkg.install
confirm  user           pkg.remove
confirm  user           svc.start
confirm  user           svc.stop
confirm  user           svc.restart
confirm  user           proc.signal
confirm  user           net.connect
confirm  user           sys.power
confirm  user           sys.mount

# --- agents get a narrower world ---------------------------------------------
allow    agent:*        ui.notify
allow    agent:*        ctx.write
allow    agent:*        media.index
allow    agent:*        desk.notify              # agents may tell you things
deny     agent:*        desk.clipboard           # but never read your clipboard
deny     agent:*        desk.screenshot          # nor watch your screen
deny     agent:*        desk.session
confirm  agent:*        desk.open
confirm  agent:*        desk.launch
confirm  agent:*        media.render:~/**
confirm  agent:*        curate.apply
confirm  agent:*        fs.write:~/**
confirm  agent:*        fs.mkdir:~/**
confirm  agent:*        fs.move:~/**
deny     agent:*        fs.write:/etc/**         # config edits go through the user
confirm  agent:*        shell.exec
confirm  agent:*        net.connect
confirm  agent:*        pkg.install
confirm  agent:*        pkg.remove
deny     agent:*        fs.delete                # agents never delete, full stop
deny     agent:*        journal.revert           # only the human rewrites history
# An agent that could "ask an assistant" could put anything it had read into
# the question. That is an exfiltration channel wearing a friendly hat.
deny     agent:*        assist.ask

# --- the daemon's own housekeeping -------------------------------------------
allow    system         fs.write:/var/lib/nous/**
allow    system         fs.write:/var/log/nous/**
allow    system         fs.mkdir:/var/lib/nous/**
allow    system         proc.spawn
"#;
        let mut p = Policy::parse(DEFAULTS, "builtin").expect("builtin policy must parse");
        p.agent_risk_ceiling = Risk::Write;
        p
    }

    pub fn empty() -> Policy {
        Policy {
            rules: Vec::new(),
            agent_risk_ceiling: Risk::Write,
        }
    }

    /// Parse a policy document. Unparseable lines are an error rather than a
    /// silent skip: a typo in a `deny` rule must not fail open.
    pub fn parse(text: &str, source: &str) -> Result<Policy, String> {
        let mut rules = Vec::new();
        for (idx, raw) in text.lines().enumerate() {
            let line_no = idx + 1;
            let line = strip_comment(raw);
            if line.trim().is_empty() {
                continue;
            }
            let comment = raw
                .find('#')
                .map(|i| raw[i + 1..].trim().to_string())
                .unwrap_or_default();
            let mut f = line.split_whitespace();
            let decision_word = f.next().unwrap_or("");
            let subject = f.next().ok_or_else(|| {
                format!(
                    "{}:{}: expected a subject after '{}'",
                    source, line_no, decision_word
                )
            })?;
            let cap_str = f.next().ok_or_else(|| {
                format!(
                    "{}:{}: expected a capability after '{}'",
                    source, line_no, subject
                )
            })?;
            if let Some(extra) = f.next() {
                return Err(format!(
                    "{}:{}: unexpected '{}' (use '#' for comments)",
                    source, line_no, extra
                ));
            }
            let reason = if comment.is_empty() {
                format!("{}:{}", source, line_no)
            } else {
                comment
            };
            let decision = match decision_word {
                "allow" => Decision::Allow,
                "confirm" => Decision::Confirm(reason),
                "deny" => Decision::Deny(reason),
                other => {
                    return Err(format!(
                        "{}:{}: unknown decision '{}' (want allow|confirm|deny)",
                        source, line_no, other
                    ))
                }
            };
            let capability =
                Capability::parse(cap_str).map_err(|e| format!("{}:{}: {}", source, line_no, e))?;
            rules.push(Rule {
                decision,
                subject: subject.to_string(),
                capability,
                source: source.to_string(),
                line: line_no,
            });
        }
        Ok(Policy {
            rules,
            agent_risk_ceiling: Risk::Write,
        })
    }

    /// Append rules from another document. Later documents take *lower*
    /// precedence than earlier ones, matching first-match-wins semantics: load
    /// site policy before the builtin defaults to let it override them.
    pub fn extend(&mut self, other: Policy) {
        self.rules.extend(other.rules);
    }

    pub fn evaluate(&self, subject: &Subject, cap: &Capability) -> Verdict {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        self.evaluate_for_home(subject, cap, &home)
    }

    /// Same, against an explicit home directory. Callers that already know it
    /// should say so; tests must, so they do not race over the environment.
    pub fn evaluate_for_home(&self, subject: &Subject, cap: &Capability, home: &str) -> Verdict {
        let risk = cap.risk();
        // Both the request and the rules are resolved against the same home
        // before they are compared.
        let cap = &cap.expand_home_with(home);

        // 1. The immutable floor. Nothing below can reach past this.
        if let Some(pattern) = protected_violation(cap) {
            return Verdict {
                decision: Decision::Deny(format!(
                    "'{}' is on the protected list ({})",
                    cap.scope, pattern
                )),
                matched: Some("protected-paths".to_string()),
                risk,
            };
        }

        // 2. Ordered rules, first match wins.
        for rule in &self.rules {
            if subject.matches(&rule.subject) && rule.capability.expand_home_with(home).covers(cap)
            {
                let matched = format!("{}:{}", rule.source, rule.line);
                // An `allow` rule cannot lift an agent past its risk ceiling; it
                // is downgraded to a confirmation instead of being honoured.
                if rule.decision.is_allow()
                    && matches!(subject, Subject::Agent(_))
                    && risk > self.agent_risk_ceiling
                {
                    return Verdict {
                        decision: Decision::Confirm(format!(
                            "{} is {} risk, above the agent ceiling of {}",
                            cap, risk, self.agent_risk_ceiling
                        )),
                        matched: Some(matched),
                        risk,
                    };
                }
                return Verdict {
                    decision: rule.decision.clone(),
                    matched: Some(matched),
                    risk,
                };
            }
        }

        // 3. Default deny. An unmatched capability is an unconsidered one.
        Verdict {
            decision: Decision::Deny(format!("no rule permits {} for {}", cap, subject.label())),
            matched: None,
            risk,
        }
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(s: &str) -> Capability {
        Capability::parse(s).unwrap()
    }

    #[test]
    fn defaults_permit_reading_your_own_files() {
        let p = Policy::builtin();
        let v = p.evaluate_for_home(
            &Subject::User,
            &cap("fs.read:/home/joey/notes.md"),
            "/home/joey",
        );
        assert_eq!(v.decision, Decision::Allow, "{}", v.explain());
    }

    #[test]
    fn the_defaults_work_for_a_home_outside_slash_home() {
        // A user provisioned under /export/home, or on a system that puts homes
        // elsewhere, must get the same defaults as everybody else.
        let home = "/export/home/joey";
        let p = Policy::builtin();
        let ok = |c: &str| {
            p.evaluate_for_home(&Subject::User, &cap(c), home)
                .decision
                .is_allow()
        };

        assert!(ok("fs.write:/export/home/joey/notes.md"));
        assert!(ok("fs.move:/export/home/joey/a.mp3"));
        // And still not into someone else's.
        assert!(!ok("fs.write:/export/home/other/x"));
    }

    #[test]
    fn deletion_always_asks_even_for_the_user() {
        let p = Policy::builtin();
        let v = p.evaluate_for_home(
            &Subject::User,
            &cap("fs.delete:/home/joey/old.txt"),
            "/home/joey",
        );
        assert!(
            matches!(v.decision, Decision::Confirm(_)),
            "{}",
            v.explain()
        );
    }

    #[test]
    fn agents_may_never_delete() {
        let p = Policy::builtin();
        let v = p.evaluate(
            &Subject::Agent("fs-agent".into()),
            &cap("fs.delete:/home/joey/x"),
        );
        assert!(matches!(v.decision, Decision::Deny(_)), "{}", v.explain());
    }

    #[test]
    fn unmatched_capabilities_default_to_deny() {
        let p = Policy::empty();
        let v = p.evaluate(&Subject::User, &cap("fs.read:/home/joey/x"));
        assert!(matches!(v.decision, Decision::Deny(_)));
        assert_eq!(v.matched, None);
    }

    #[test]
    fn protected_paths_beat_an_explicit_allow() {
        let mut p = Policy::parse("allow user fs.write:/**", "test").unwrap();
        p.extend(Policy::builtin());
        let v = p.evaluate(&Subject::User, &cap("fs.write:/boot/grub/grub.cfg"));
        assert!(matches!(v.decision, Decision::Deny(_)), "{}", v.explain());
        assert_eq!(v.matched.as_deref(), Some("protected-paths"));
        // ...but an ordinary path under the same rule is fine.
        assert!(p
            .evaluate(&Subject::User, &cap("fs.write:/srv/data"))
            .decision
            .is_allow());
    }

    #[test]
    fn secrets_never_reach_context() {
        let p = Policy::builtin();
        for path in [
            "/etc/shadow",
            "/home/joey/.ssh/id_ed25519",
            "/home/joey/.aws/credentials",
        ] {
            let v = p.evaluate(&Subject::User, &cap(&format!("fs.read:{}", path)));
            assert!(
                matches!(v.decision, Decision::Deny(_)),
                "{} should be denied",
                path
            );
        }
    }

    #[test]
    fn agent_risk_ceiling_downgrades_allow_to_confirm() {
        let mut p = Policy::parse("allow * pkg.install", "test").unwrap();
        p.agent_risk_ceiling = Risk::Write;
        // pkg.install is Elevated, above the ceiling, so the agent must ask...
        let v = p.evaluate(&Subject::Agent("sys-agent".into()), &cap("pkg.install:vim"));
        assert!(
            matches!(v.decision, Decision::Confirm(_)),
            "{}",
            v.explain()
        );
        // ...while the same rule is honoured as written for the human.
        let u = p.evaluate(&Subject::User, &cap("pkg.install:vim"));
        assert!(u.decision.is_allow(), "{}", u.explain());
    }

    #[test]
    fn first_match_wins_in_document_order() {
        let p = Policy::parse(
            "deny  user  fs.write:/home/joey/locked/**\nallow user  fs.write:/home/**",
            "test",
        )
        .unwrap();
        assert!(matches!(
            p.evaluate(&Subject::User, &cap("fs.write:/home/joey/locked/a"))
                .decision,
            Decision::Deny(_)
        ));
        assert!(p
            .evaluate(&Subject::User, &cap("fs.write:/home/joey/free/a"))
            .decision
            .is_allow());
    }

    #[test]
    fn malformed_policy_is_an_error_not_a_skip() {
        assert!(Policy::parse("permit user fs.read", "t").is_err());
        assert!(Policy::parse("allow user", "t").is_err());
        assert!(Policy::parse("allow user fs.read extra", "t").is_err());
        assert!(Policy::parse("allow user notacap", "t").is_err());
    }

    #[test]
    fn comments_become_the_stated_reason() {
        let p = Policy::parse("deny user shell.exec  # not on this box", "t").unwrap();
        let v = p.evaluate(&Subject::User, &cap("shell.exec:rm"));
        assert_eq!(v.decision.reason(), "not on this box");
    }
}
