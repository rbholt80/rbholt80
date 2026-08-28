//! What is being asked for.
//!
//! Every action a caller wants to take is first written down as a
//! `Capability` — a domain, an action, and a scope — before anything happens.
//! Writing it down is not bureaucracy: it is what makes the request something
//! a fixed rule can be applied to, instead of something a model has to be
//! trusted about.
//!
//! A capability is written `domain.action:scope`:
//!
//! ```text
//! fs.write:/home/robert/**
//! http.post:api.stripe.com
//! db.delete:users
//! ```
//!
//! Scope is optional and defaults to `*`. Warrant attaches no meaning to any
//! particular domain or action — `fs`, `http` and `db` above are just strings
//! this host chose. The host says what they mean, and grades them
//! ([`crate::Grades`]).

use std::fmt;

/// A request, in the form a rule can be applied to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Capability {
    pub domain: String,
    pub action: String,
    pub scope: String,
}

impl Capability {
    pub fn new(domain: &str, action: &str, scope: &str) -> Capability {
        Capability {
            domain: domain.to_string(),
            action: action.to_string(),
            scope: if scope.is_empty() {
                "*".to_string()
            } else {
                scope.to_string()
            },
        }
    }

    /// Parse `domain.action:scope`. Scope may be omitted.
    ///
    /// A scope may itself contain `:` — `http.get:https://example.com/x` is one
    /// capability, not a parse error — so only the first colon divides.
    pub fn parse(s: &str) -> Result<Capability, String> {
        let (head, scope) = match s.find(':') {
            Some(i) => (&s[..i], &s[i + 1..]),
            None => (s, "*"),
        };
        let mut parts = head.splitn(2, '.');
        let domain = parts.next().unwrap_or("").trim();
        let action = parts.next().unwrap_or("").trim();
        if domain.is_empty() || action.is_empty() {
            return Err(format!(
                "malformed capability '{}': want domain.action[:scope]",
                s
            ));
        }
        Ok(Capability::new(domain, action, scope.trim()))
    }

    /// `domain.action`, without the scope. This is what a grade is looked up by.
    pub fn name(&self) -> String {
        format!("{}.{}", self.domain, self.action)
    }

    /// Resolve a leading `~` in the scope against `home`.
    ///
    /// Both a rule and the request it is compared against are expanded with the
    /// same home before they meet, so `~/notes` in a policy file means the same
    /// thing as the absolute path a caller actually asks for.
    pub fn expand_home(&self, home: &str) -> Capability {
        let scope = if self.scope == "~" {
            home.to_string()
        } else if let Some(rest) = self.scope.strip_prefix("~/") {
            format!("{}/{}", home.trim_end_matches('/'), rest)
        } else {
            return self.clone();
        };
        Capability {
            domain: self.domain.clone(),
            action: self.action.clone(),
            scope,
        }
    }

    /// Does this capability, read as a *grant*, cover `req`, read as a
    /// *request*?
    ///
    /// Grants may use `*` for domain or action and glob syntax in scope.
    /// Requests are concrete. The asymmetry is deliberate: a request that
    /// arrives with a wildcard in it is a request to do an unbounded number of
    /// things, and [`Capability::is_concrete`] exists so a host can refuse it.
    pub fn covers(&self, req: &Capability) -> bool {
        wild_eq(&self.domain, &req.domain)
            && wild_eq(&self.action, &req.action)
            && glob_match(&self.scope, &req.scope)
    }

    /// True if this names one specific thing — no wildcards anywhere.
    ///
    /// A caller asking for `fs.delete:/home/**` is not asking to delete a file,
    /// it is asking for the authority to delete any file. Those deserve
    /// different answers, and a host that cannot tell them apart will
    /// eventually give the second one by accident.
    pub fn is_concrete(&self) -> bool {
        !has_wildcard(&self.domain) && !has_wildcard(&self.action) && !has_wildcard(&self.scope)
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.scope == "*" {
            write!(f, "{}.{}", self.domain, self.action)
        } else {
            write!(f, "{}.{}:{}", self.domain, self.action, self.scope)
        }
    }
}

fn has_wildcard(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

fn wild_eq(pattern: &str, actual: &str) -> bool {
    pattern == "*" || pattern == actual
}

/// Glob matcher supporting `?`, `*` (within one path segment) and `**` (any
/// depth).
///
/// Written iteratively with backtracking rather than recursively, so that a
/// pathological pattern from a config file cannot blow the stack of the process
/// that is deciding whether to trust it.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" || pattern == "**" || pattern == text {
        return true;
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();

    let (mut pi, mut ti) = (0usize, 0usize);
    // Backtrack point for the most recent `*` / `**`.
    let mut star_p: Option<usize> = None;
    let mut star_t = 0usize;
    let mut star_crosses_sep = false;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            let double = pi + 1 < p.len() && p[pi + 1] == '*';
            star_crosses_sep = double;
            pi += if double { 2 } else { 1 };
            // `**/` should match zero segments too, so swallow a following slash.
            if double && pi < p.len() && p[pi] == '/' {
                pi += 1;
            }
            star_p = Some(pi);
            star_t = ti;
        } else if let Some(sp) = star_p {
            // Let the star consume one more character — unless it is a single
            // `*`, which may not cross a path separator.
            if !star_crosses_sep && t[star_t] == '/' {
                return false;
            }
            star_t += 1;
            ti = star_t;
            pi = sp;
        } else {
            return false;
        }
    }

    // Trailing wildcards may match nothing at all. `/**` has to be skipped
    // together with its leading slash, or `/home/**` fails to cover `/home` —
    // and a policy granting `~/**` would then refuse to list your own home
    // directory, which is not what anybody writing that line meant.
    while pi < p.len() {
        if p[pi] == '*' {
            pi += 1;
        } else if p[pi] == '/' && p.len() >= pi + 3 && p[pi + 1] == '*' && p[pi + 2] == '*' {
            pi += 3;
        } else {
            break;
        }
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(s: &str) -> Capability {
        Capability::parse(s).unwrap()
    }

    #[test]
    fn parses_the_three_forms() {
        let c = cap("fs.write:/home/**");
        assert_eq!((c.domain.as_str(), c.action.as_str()), ("fs", "write"));
        assert_eq!(c.scope, "/home/**");

        assert_eq!(cap("proc.list").scope, "*");
        assert_eq!(cap("proc.list").to_string(), "proc.list");
    }

    #[test]
    fn a_scope_may_contain_colons() {
        // A URL scope is the obvious case, and splitting on the last colon or
        // on every colon both get it wrong.
        let c = cap("http.get:https://example.com:8443/x");
        assert_eq!(c.name(), "http.get");
        assert_eq!(c.scope, "https://example.com:8443/x");
    }

    #[test]
    fn rejects_malformed_capabilities() {
        assert!(Capability::parse("fs").is_err());
        assert!(Capability::parse("").is_err());
        assert!(Capability::parse(".write").is_err());
        assert!(Capability::parse("fs.").is_err());
    }

    #[test]
    fn a_grant_covers_what_it_globs() {
        assert!(cap("fs.write:/home/**").covers(&cap("fs.write:/home/r/a/b.txt")));
        assert!(cap("fs.*:/tmp/**").covers(&cap("fs.delete:/tmp/x")));
        assert!(!cap("fs.write:/home/**").covers(&cap("fs.write:/etc/passwd")));
    }

    #[test]
    fn a_single_star_does_not_cross_a_separator() {
        // This is the whole reason `**` exists. If `*` spanned separators, every
        // policy written for one directory would silently cover the tree.
        assert!(cap("fs.read:/home/*").covers(&cap("fs.read:/home/notes")));
        assert!(!cap("fs.read:/home/*").covers(&cap("fs.read:/home/r/.ssh/id_rsa")));
        assert!(cap("fs.read:/home/**").covers(&cap("fs.read:/home/r/.ssh/id_rsa")));
    }

    #[test]
    fn double_star_matches_zero_segments() {
        assert!(glob_match("/**/.ssh/**", "/home/r/.ssh/id_rsa"));
        assert!(glob_match("/home/**", "/home/"));
    }

    #[test]
    fn a_tree_grant_covers_the_root_of_the_tree() {
        // `allow user fs.list:~/**` has to cover listing the home directory
        // itself. Requiring a second rule for the root of your own tree is the
        // kind of gap that gets papered over with a broader grant.
        assert!(glob_match("/home/**", "/home"));
        assert!(cap("fs.list:~/**")
            .expand_home("/home/robert")
            .covers(&cap("fs.list:/home/robert")));
        // ...without the pattern leaking sideways into a sibling.
        assert!(!glob_match("/home/**", "/homework"));
        assert!(!glob_match("/home/**", "/home2/x"));
    }

    #[test]
    fn a_request_that_wildcards_is_not_concrete() {
        assert!(cap("fs.delete:/home/r/old.txt").is_concrete());
        assert!(!cap("fs.delete:/home/**").is_concrete());
        assert!(!cap("fs.*:/home/r/old.txt").is_concrete());
        assert!(!cap("fs.delete").is_concrete()); // scope defaulted to `*`
    }

    #[test]
    fn tilde_expands_on_both_sides_of_a_comparison() {
        let grant = cap("fs.read:~/notes/**").expand_home("/home/robert");
        let req = cap("fs.read:~/notes/a.md").expand_home("/home/robert");
        assert_eq!(req.scope, "/home/robert/notes/a.md");
        assert!(grant.covers(&req));
    }

    #[test]
    fn a_pathological_pattern_terminates() {
        // Backtracking globs are where stack overflows and hangs live. This is
        // the shape that kills a naive recursive matcher.
        let pattern = "/**/**/**/**/**/**/x";
        assert!(!glob_match(pattern, &format!("/{}/y", "a/".repeat(40))));
    }
}
