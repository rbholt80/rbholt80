//! How much damage a thing can do.
//!
//! Risk is a property of the *capability*, never of the requester and never of
//! how the request was phrased. `fs.delete` is dangerous when a model asks for
//! it and equally dangerous when you ask for it; what changes between those two
//! cases is the answer, not the risk.
//!
//! The grades themselves come from the host, as data, in a file a person can
//! read:
//!
//! ```text
//! read      fs.read fs.list fs.stat
//! write     fs.write fs.move
//! elevated  fs.delete http.post
//! critical  sys.firmware db.drop
//! ```
//!
//! Warrant refuses to guess. An ungraded capability is [`Risk::Critical`], on
//! the grounds that a thing nobody has thought about is exactly the thing you
//! should be asked about — and that a system which silently defaults unknowns
//! to "harmless" will be wrong precisely once.

use std::collections::BTreeMap;
use std::fmt;

use crate::cap::Capability;

/// How much damage a capability can do if the request was wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    /// Observation only. Cannot change any state.
    Read = 0,
    /// Changes state, but reversibly and locally.
    Write = 1,
    /// Hard to undo, or reaches outside this machine.
    Elevated = 2,
    /// Destroys data at scale, breaks the boot path, or leaks secrets.
    Critical = 3,
}

impl Risk {
    pub fn as_str(&self) -> &'static str {
        match self {
            Risk::Read => "read",
            Risk::Write => "write",
            Risk::Elevated => "elevated",
            Risk::Critical => "critical",
        }
    }

    pub fn parse(s: &str) -> Option<Risk> {
        match s {
            "read" => Some(Risk::Read),
            "write" => Some(Risk::Write),
            "elevated" => Some(Risk::Elevated),
            "critical" => Some(Risk::Critical),
            _ => None,
        }
    }
}

impl fmt::Display for Risk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The host's table of what its own capabilities cost.
///
/// This is deliberately a table and not a heuristic. Somebody auditing a system
/// should be able to answer "what does this consider dangerous?" by reading one
/// file, not by reasoning about code.
#[derive(Debug, Clone, Default)]
pub struct Grades {
    by_name: BTreeMap<String, Risk>,
}

impl Grades {
    pub fn new() -> Grades {
        Grades {
            by_name: BTreeMap::new(),
        }
    }

    /// Grade one `domain.action`. A trailing `*` action is allowed, so
    /// `fs.*` grades a whole domain in one line.
    pub fn set(&mut self, name: &str, risk: Risk) -> &mut Grades {
        self.by_name.insert(name.to_string(), risk);
        self
    }

    /// Parse a grades document. Lines are `<risk> <name> [name...]`, `#` comments.
    pub fn parse(text: &str, source: &str) -> Result<Grades, String> {
        let mut g = Grades::new();
        for (idx, raw) in text.lines().enumerate() {
            let line = strip_comment(raw);
            if line.trim().is_empty() {
                continue;
            }
            let mut f = line.split_whitespace();
            let word = f.next().unwrap_or("");
            let risk = Risk::parse(word).ok_or_else(|| {
                format!(
                    "{}:{}: unknown risk '{}' (want read|write|elevated|critical)",
                    source,
                    idx + 1,
                    word
                )
            })?;
            let mut any = false;
            for name in f {
                if !name.contains('.') {
                    return Err(format!(
                        "{}:{}: '{}' is not a domain.action",
                        source,
                        idx + 1,
                        name
                    ));
                }
                g.set(name, risk);
                any = true;
            }
            if !any {
                return Err(format!(
                    "{}:{}: '{}' grades nothing — name at least one capability",
                    source,
                    idx + 1,
                    word
                ));
            }
        }
        Ok(g)
    }

    /// The grade for a capability. Exact `domain.action` first, then `domain.*`.
    ///
    /// Anything ungraded is [`Risk::Critical`]. See the module note: this is the
    /// single most important default in the crate, and it errs loud.
    pub fn risk(&self, cap: &Capability) -> Risk {
        let name = cap.name();
        if let Some(r) = self.by_name.get(&name) {
            return *r;
        }
        if let Some(r) = self.by_name.get(&format!("{}.*", cap.domain)) {
            return *r;
        }
        Risk::Critical
    }

    /// True if this capability was actually graded, rather than falling to the
    /// unknown default. A host can use this to reject a plan up front instead of
    /// discovering at the last step that it was never considered.
    pub fn is_known(&self, cap: &Capability) -> bool {
        self.by_name.contains_key(&cap.name())
            || self.by_name.contains_key(&format!("{}.*", cap.domain))
    }

    /// Every capability graded here, for a host that wants to print its own
    /// surface area.
    pub fn known(&self) -> Vec<(String, Risk)> {
        self.by_name.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
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

    fn grades() -> Grades {
        Grades::parse(
            "read      fs.read fs.list\n\
             write     fs.write\n\
             elevated  fs.delete\n\
             critical  sys.firmware\n",
            "test",
        )
        .unwrap()
    }

    #[test]
    fn grades_what_it_was_told() {
        let g = grades();
        assert_eq!(g.risk(&cap("fs.read:/x")), Risk::Read);
        assert_eq!(g.risk(&cap("fs.write:/x")), Risk::Write);
        assert_eq!(g.risk(&cap("fs.delete:/x")), Risk::Elevated);
        assert_eq!(g.risk(&cap("sys.firmware")), Risk::Critical);
    }

    #[test]
    fn an_ungraded_capability_is_critical_not_harmless() {
        // The defining default of the crate. If this ever flips to Read, a
        // host that adds a capability and forgets to grade it gets it for free.
        let g = grades();
        assert!(!g.is_known(&cap("db.drop:users")));
        assert_eq!(g.risk(&cap("db.drop:users")), Risk::Critical);
    }

    #[test]
    fn a_domain_wildcard_grades_the_rest_of_the_domain() {
        let mut g = grades();
        g.set("net.*", Risk::Elevated);
        assert_eq!(g.risk(&cap("net.connect:example.com")), Risk::Elevated);
        // ...but an exact grade still wins over the domain default.
        g.set("net.status", Risk::Read);
        assert_eq!(g.risk(&cap("net.status")), Risk::Read);
        assert_eq!(g.risk(&cap("net.connect:example.com")), Risk::Elevated);
    }

    #[test]
    fn risks_order_by_severity() {
        assert!(Risk::Read < Risk::Write);
        assert!(Risk::Write < Risk::Elevated);
        assert!(Risk::Elevated < Risk::Critical);
    }

    #[test]
    fn a_line_that_grades_nothing_is_an_error() {
        // "elevated" alone reads like it did something. It did not, and a
        // silently-ignored line in a security file is how holes are made.
        assert!(Grades::parse("elevated\n", "t").is_err());
        assert!(Grades::parse("read fsread\n", "t").is_err());
        assert!(Grades::parse("dangerous fs.read\n", "t").is_err());
    }

    #[test]
    fn comments_and_blank_lines_are_fine() {
        let g = Grades::parse("# what we allow\n\nread fs.read # observation\n", "t").unwrap();
        assert_eq!(g.risk(&cap("fs.read:/x")), Risk::Read);
        assert_eq!(g.known().len(), 1);
    }
}
