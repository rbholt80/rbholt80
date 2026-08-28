//! The rules, and the one question they answer.
//!
//! Warrant asks policy exactly one thing — *may this subject exercise this
//! capability?* — and policy answers `allow`, `confirm` or `deny`. Rules are
//! ordered and the first match wins, which is what makes a policy file readable
//! top to bottom: narrow exceptions above broad defaults.
//!
//! ```text
//! # decision  subject         capability            # reason
//! never       *               fs.read:/**/.ssh/**   # keys never leave the disk
//! deny        agent:*         pkg.install           # ask a human, not me
//! confirm     agent:*         fs.delete:~/**        # say it out loud first
//! allow       user            fs.read:~/**
//! ```
//!
//! `never` is not a louder `deny`. A `deny` is a rule like any other and a rule
//! above it can win; a `never` is a floor, checked before the ordered rules run,
//! that nothing later in the file and no caller-supplied override can lift. It
//! is where you put the things that are true regardless of how good the argument
//! for the exception sounds — which, when the argument is being written by a
//! language model, is the category that matters.

use crate::cap::Capability;
use crate::grade::{Grades, Risk};

/// What policy says about a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Proceed without asking.
    Allow,
    /// Proceed only after a human says yes.
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

/// Who is asking.
///
/// The distinction that matters is not *which* agent but *whether* the thing
/// asking is one. A human typing a command has already decided; a program
/// proposing one has not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// A person, present, who will see the consequences.
    User,
    /// Code acting on its own account — a model, a script, a bot.
    Agent(String),
    /// The host itself, doing its own housekeeping.
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
        match s.trim() {
            "user" => Subject::User,
            "system" => Subject::System,
            other => Subject::Agent(other.trim_start_matches("agent:").to_string()),
        }
    }

    pub fn is_agent(&self) -> bool {
        matches!(self, Subject::Agent(_))
    }

    fn matches(&self, pattern: &str) -> bool {
        match pattern {
            "*" => true,
            "agent:*" => self.is_agent(),
            p => p == self.label(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub decision: Decision,
    /// True for a `never` line: a floor, not an ordered rule.
    pub absolute: bool,
    pub subject: String,
    pub capability: Capability,
    pub source: String,
    pub line: usize,
}

impl Rule {
    /// `source:line`, which is what a verdict cites.
    pub fn cite(&self) -> String {
        format!("{}:{}", self.source, self.line)
    }
}

/// The answer, with enough provenance to defend itself.
///
/// A verdict that cannot say *which line* decided it is not auditable, and an
/// authorization layer that is not auditable is decoration.
#[derive(Debug, Clone)]
pub struct Verdict {
    pub decision: Decision,
    /// The rule that decided, as `source:line`. `None` means the default deny.
    pub matched: Option<String>,
    pub risk: Risk,
    /// True when a `never` line decided this. Nothing can lift it.
    pub absolute: bool,
}

impl Verdict {
    pub fn is_allow(&self) -> bool {
        self.decision.is_allow()
    }

    pub fn explain(&self) -> String {
        let src = self
            .matched
            .clone()
            .unwrap_or_else(|| "default".to_string());
        match &self.decision {
            Decision::Allow => format!("allowed by {} (risk: {})", src, self.risk),
            Decision::Confirm(r) => format!("needs confirmation — {} [{}]", r, src),
            Decision::Deny(r) if self.absolute => format!("forbidden — {} [{}]", r, src),
            Decision::Deny(r) => format!("denied — {} [{}]", r, src),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Policy {
    pub rules: Vec<Rule>,
    /// An `allow` rule cannot lift an agent above this. It is downgraded to a
    /// confirmation instead of being honoured.
    ///
    /// This exists because policy files are written once and capabilities are
    /// added forever. A broad `allow agent:* fs.*` written when `fs` meant
    /// reading should not silently start authorising a `fs.wipe` added later.
    pub agent_ceiling: Risk,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            rules: Vec::new(),
            agent_ceiling: Risk::Write,
        }
    }
}

impl Policy {
    /// An empty policy. Denies everything, which is the correct thing for a
    /// system with no rules to do.
    pub fn empty() -> Policy {
        Policy::default()
    }

    /// Parse a policy document.
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
            let word = f.next().unwrap_or("");
            let subject = f.next().ok_or_else(|| {
                format!(
                    "{}:{}: expected a subject after '{}'",
                    source, line_no, word
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
            let (decision, absolute) = match word {
                "allow" => (Decision::Allow, false),
                "confirm" => (Decision::Confirm(reason), false),
                "deny" => (Decision::Deny(reason), false),
                "never" => (Decision::Deny(reason), true),
                other => {
                    return Err(format!(
                        "{}:{}: unknown decision '{}' (want allow|confirm|deny|never)",
                        source, line_no, other
                    ))
                }
            };
            let capability =
                Capability::parse(cap_str).map_err(|e| format!("{}:{}: {}", source, line_no, e))?;
            rules.push(Rule {
                decision,
                absolute,
                subject: subject.to_string(),
                capability,
                source: source.to_string(),
                line: line_no,
            });
        }
        Ok(Policy {
            rules,
            agent_ceiling: Risk::Write,
        })
    }

    /// Append rules from another document.
    ///
    /// Later documents take *lower* precedence, matching first-match-wins: load
    /// the site's policy before the defaults so it can override them. `never`
    /// lines are unaffected by order — they are a floor wherever they appear.
    pub fn extend(&mut self, other: Policy) {
        self.rules.extend(other.rules);
    }

    /// Every `never` line, so a host can show what it will not do under any
    /// circumstances.
    pub fn absolutes(&self) -> Vec<&Rule> {
        self.rules.iter().filter(|r| r.absolute).collect()
    }

    /// Answer the one question.
    pub fn evaluate(
        &self,
        subject: &Subject,
        cap: &Capability,
        grades: &Grades,
        home: &str,
    ) -> Verdict {
        let risk = grades.risk(cap);
        // Rules and request are resolved against the same home before comparison.
        let cap = &cap.expand_home(home);

        // 1. The floor. Checked first, so no ordered rule can get underneath it.
        for rule in self.rules.iter().filter(|r| r.absolute) {
            if rule.subject_matches(subject) && rule.capability.expand_home(home).covers(cap) {
                return Verdict {
                    decision: rule.decision.clone(),
                    matched: Some(rule.cite()),
                    risk,
                    absolute: true,
                };
            }
        }

        // 2. Ordered rules, first match wins.
        for rule in self.rules.iter().filter(|r| !r.absolute) {
            if !rule.subject_matches(subject) || !rule.capability.expand_home(home).covers(cap) {
                continue;
            }
            if rule.decision.is_allow() && subject.is_agent() && risk > self.agent_ceiling {
                return Verdict {
                    decision: Decision::Confirm(format!(
                        "{} is {} risk, above the agent ceiling of {}",
                        cap, risk, self.agent_ceiling
                    )),
                    matched: Some(rule.cite()),
                    risk,
                    absolute: false,
                };
            }
            return Verdict {
                decision: rule.decision.clone(),
                matched: Some(rule.cite()),
                risk,
                absolute: false,
            };
        }

        // 3. Default deny. An unmatched capability is an unconsidered one.
        Verdict {
            decision: Decision::Deny(format!("no rule permits {} for {}", cap, subject.label())),
            matched: None,
            risk,
            absolute: false,
        }
    }
}

impl Rule {
    fn subject_matches(&self, subject: &Subject) -> bool {
        subject.matches(&self.subject)
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

    const HOME: &str = "/home/robert";

    fn cap(s: &str) -> Capability {
        Capability::parse(s).unwrap()
    }

    fn grades() -> Grades {
        Grades::parse(
            "read fs.read fs.list\nwrite fs.write\nelevated fs.delete pkg.install\n",
            "grades",
        )
        .unwrap()
    }

    fn policy(text: &str) -> Policy {
        Policy::parse(text, "policy").unwrap()
    }

    fn verdict(p: &Policy, s: &str, c: &str) -> Verdict {
        p.evaluate(&Subject::parse(s), &cap(c), &grades(), HOME)
    }

    #[test]
    fn first_matching_rule_wins() {
        let p = policy(
            "deny  agent:* fs.read:~/.ssh/**\n\
             allow agent:* fs.read:~/**\n",
        );
        assert!(!verdict(&p, "agent:a", "fs.read:/home/robert/.ssh/id_rsa").is_allow());
        assert!(verdict(&p, "agent:a", "fs.read:/home/robert/notes.md").is_allow());
    }

    #[test]
    fn nothing_permits_what_no_rule_mentions() {
        let p = policy("allow user fs.read:~/**\n");
        let v = verdict(&p, "user", "fs.delete:/home/robert/notes.md");
        assert_eq!(v.decision.kind(), "deny");
        assert_eq!(v.matched, None, "the default deny cites no rule");
    }

    #[test]
    fn an_empty_policy_denies_everything() {
        let p = Policy::empty();
        assert_eq!(verdict(&p, "user", "fs.read:/x").decision.kind(), "deny");
    }

    #[test]
    fn never_beats_an_allow_written_above_it() {
        // The point of `never`. Ordered-rule semantics would let the allow win,
        // because it comes first. It must not.
        let p = policy(
            "allow user  fs.read:~/**            # broad, and written first\n\
             never *     fs.read:/**/.ssh/**     # and still no\n",
        );
        let v = verdict(&p, "user", "fs.read:/home/robert/.ssh/id_rsa");
        assert_eq!(v.decision.kind(), "deny", "{}", v.explain());
        assert!(v.absolute);
        assert_eq!(v.matched.as_deref(), Some("policy:2"));

        // And it is surgical: the broad allow still works for everything else.
        assert!(verdict(&p, "user", "fs.read:/home/robert/notes.md").is_allow());
    }

    #[test]
    fn never_binds_the_user_too_not_only_agents() {
        let p = policy("never * fs.delete:/boot/**\nallow user fs.delete:/**\n");
        assert!(!verdict(&p, "user", "fs.delete:/boot/vmlinuz").is_allow());
    }

    #[test]
    fn an_allow_cannot_lift_an_agent_past_the_ceiling() {
        // A policy written when `fs` meant reading should not authorise
        // whatever `fs` grows into later.
        let p = policy("allow agent:* fs.*:~/**\n");
        let v = verdict(&p, "agent:a", "fs.delete:/home/robert/notes.md");
        assert_eq!(v.decision.kind(), "confirm", "{}", v.explain());
        // The same rule, for the same agent, is honoured below the ceiling.
        assert!(verdict(&p, "agent:a", "fs.write:/home/robert/notes.md").is_allow());
    }

    #[test]
    fn the_ceiling_does_not_bind_a_present_human() {
        let p = policy("allow user fs.*:~/**\n");
        assert!(verdict(&p, "user", "fs.delete:/home/robert/notes.md").is_allow());
    }

    #[test]
    fn an_ungraded_capability_lands_above_the_ceiling() {
        // Grades default to Critical, so a capability nobody graded cannot be
        // handed to an agent by a broad allow. This is the two defaults
        // working together, and it is the reason both are set the way they are.
        let p = policy("allow agent:* db.*\n");
        let v = p.evaluate(
            &Subject::parse("agent:a"),
            &cap("db.drop:users"),
            &grades(),
            HOME,
        );
        assert_eq!(v.decision.kind(), "confirm", "{}", v.explain());
        assert_eq!(v.risk, Risk::Critical);
    }

    #[test]
    fn a_verdict_names_the_line_that_decided_it() {
        let p = policy("# header\n\nallow user fs.read:~/**\n");
        assert_eq!(
            verdict(&p, "user", "fs.read:/home/robert/a")
                .matched
                .as_deref(),
            Some("policy:3")
        );
    }

    #[test]
    fn a_comment_becomes_the_reason_a_human_is_shown() {
        let p = policy("confirm agent:* fs.delete:~/** # this cannot be taken back\n");
        let v = verdict(&p, "agent:a", "fs.delete:/home/robert/a");
        assert_eq!(v.decision.reason(), "this cannot be taken back");
    }

    #[test]
    fn agent_star_matches_any_agent_but_not_the_user() {
        let p = policy("deny agent:* fs.read:~/**\nallow user fs.read:~/**\n");
        assert!(!verdict(&p, "agent:anything", "fs.read:/home/robert/a").is_allow());
        assert!(verdict(&p, "user", "fs.read:/home/robert/a").is_allow());
    }

    #[test]
    fn later_documents_lose_to_earlier_ones() {
        let mut site = policy("deny agent:* fs.write:~/**\n");
        site.extend(Policy::parse("allow agent:* fs.write:~/**\n", "defaults").unwrap());
        assert!(!verdict(&site, "agent:a", "fs.write:/home/robert/a").is_allow());
    }

    #[test]
    fn a_never_in_a_later_document_still_binds() {
        // Precedence is about ordered rules. A floor loaded second is still a floor.
        let mut site = policy("allow user fs.read:~/**\n");
        site.extend(Policy::parse("never * fs.read:/**/.ssh/**\n", "defaults").unwrap());
        assert!(!verdict(&site, "user", "fs.read:/home/robert/.ssh/id_rsa").is_allow());
    }

    #[test]
    fn malformed_lines_are_rejected_loudly() {
        assert!(Policy::parse("allow\n", "t").is_err());
        assert!(Policy::parse("allow user\n", "t").is_err());
        assert!(Policy::parse("maybe user fs.read\n", "t").is_err());
        assert!(Policy::parse("allow user fs.read extra\n", "t").is_err());
        assert!(Policy::parse("allow user notacap\n", "t").is_err());
    }

    #[test]
    fn absolutes_can_be_listed() {
        let p =
            policy("never * fs.read:/**/.ssh/**\nallow user fs.read:~/**\nnever * sys.firmware\n");
        let lines: Vec<usize> = p.absolutes().iter().map(|r| r.line).collect();
        assert_eq!(lines, vec![1, 3]);
    }
}
