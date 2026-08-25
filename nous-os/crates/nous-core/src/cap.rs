//! Capability model.
//!
//! Nothing in NOUS OS acts on the machine directly. Every effect an agent or a
//! model wants to have is first expressed as a `Capability` — a domain, an
//! action, and a scope — and handed to the broker, which consults policy.
//!
//! A capability is written `domain.action:scope`, e.g. `fs.write:/home/**`.
//! Scope is optional and defaults to `*` (everything the domain covers).

use std::fmt;

/// How much damage a capability can do if the request was wrong.
///
/// Risk is a property of the *capability*, not of the requester. It is what the
/// policy engine and the UI use to decide how loudly to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    /// Observation only. Cannot change system state.
    Read = 0,
    /// Changes state, but reversibly and locally (write a file, start a service).
    Write = 1,
    /// Hard to undo, or reaches outside the machine (delete, network, packages).
    Elevated = 2,
    /// Can break the boot path, destroy data at scale, or exfiltrate secrets.
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
}

impl fmt::Display for Risk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

    pub fn name(&self) -> String {
        format!("{}.{}", self.domain, self.action)
    }

    /// The intrinsic risk of this capability.
    ///
    /// Deliberately a table and not a heuristic: an operator reading this file
    /// should be able to see exactly what the system considers dangerous.
    pub fn risk(&self) -> Risk {
        match (self.domain.as_str(), self.action.as_str()) {
            // Observation
            ("fs", "read") | ("fs", "stat") | ("fs", "list") | ("fs", "search") => Risk::Read,
            ("proc", "list") | ("sys", "info") | ("sys", "metrics") => Risk::Read,
            ("svc", "status") | ("net", "status") | ("pkg", "query") => Risk::Read,
            ("ctx", "read") | ("journal", "read") => Risk::Read,
            ("media", "probe") | ("media", "search") | ("media", "thumbnail") => Risk::Read,
            // Looking at the desktop is free; touching it is not.
            ("desk", "apps") | ("desk", "windows") | ("desk", "session_info") => Risk::Read,

            // Local, reversible mutation
            ("fs", "write") | ("fs", "mkdir") | ("fs", "move") => Risk::Write,
            ("ctx", "write") | ("ui", "notify") | ("ui", "render") => Risk::Write,
            ("svc", "start") | ("svc", "stop") | ("svc", "restart") => Risk::Write,
            // Undo changes state, so it is a write -- but see the policy: it is
            // allowed outright, because undo must never be harder to reach than
            // the action it reverses.
            ("journal", "revert") => Risk::Write,
            ("fs", "index") => Risk::Write,
            ("model", "infer") => Risk::Write,
            // Playback and edit-graph authoring touch nothing on disk; rendering
            // writes a new file and never overwrites its source.
            ("media", "play") | ("media", "control") | ("media", "edit") => Risk::Write,
            ("media", "render") | ("media", "index") => Risk::Write,
            // The curator proposes; applying its proposal is an ordinary write.
            ("curate", "scan") | ("curate", "propose") => Risk::Read,
            ("curate", "apply") => Risk::Write,

            // Desktop actions that only affect what you can already see.
            ("desk", "notify") | ("desk", "copy") | ("desk", "focus") => Risk::Write,
            ("desk", "open") => Risk::Write,

            // Hard to undo, or leaves the machine
            ("fs", "delete") | ("fs", "chmod") | ("fs", "chown") => Risk::Elevated,
            ("proc", "signal") | ("proc", "spawn") => Risk::Elevated,
            ("net", "connect") | ("net", "listen") => Risk::Elevated,
            // Reading the clipboard and capturing the screen are elevated for
            // the same reason: whatever is there right now may be a password, a
            // private message, or someone else's data, and it would be crossing
            // an inference boundary. Neither is a "read" in the harmless sense.
            ("desk", "clipboard") | ("desk", "screenshot") => Risk::Elevated,
            // Asking a hosted assistant sends what you typed to a third party.
            // That is elevated whatever the convenience, and the label says so
            // even though the default policy lets it run without a prompt --
            // configuring a key is the consent, but it should still be visible.
            ("assist", "ask") | ("assist", "list") => Risk::Elevated,
            // Launching an arbitrary application, closing a window that may
            // hold unsaved work, and changing desktop settings.
            ("desk", "launch") | ("desk", "close") | ("desk", "setting") => Risk::Elevated,
            ("pkg", "install") | ("pkg", "remove") => Risk::Elevated,
            ("shell", "exec") => Risk::Elevated,

            // Can end the machine or the user's privacy
            ("sys", "power") | ("sys", "mount") | ("sys", "firmware") => Risk::Critical,
            // Locking or ending the session can lose work across every open
            // application at once.
            ("desk", "session") => Risk::Critical,
            ("policy", "amend") | ("secret", "read") | ("user", "admin") => Risk::Critical,

            // Unknown capabilities are treated as critical, never as safe.
            _ => Risk::Critical,
        }
    }

    /// Resolve `~` in the scope against the current home directory.
    ///
    /// Policy is written with `~/**` rather than `/home/**` because a home
    /// directory is not always under `/home` -- it differs on macOS, on systems
    /// with `/export/home`, for the root account, and for anyone whose account
    /// was provisioned somewhere else. Both grants and requests are normalised
    /// before they are compared, so the two sides always agree.
    pub fn expand_home(&self) -> Capability {
        match std::env::var("HOME") {
            Ok(h) if !h.is_empty() => self.expand_home_with(&h),
            _ => self.clone(),
        }
    }

    /// Same, against an explicit home directory.
    ///
    /// Separate from `expand_home` so callers that already know the home -- and
    /// tests, which must not race each other over a process-global environment
    /// variable -- can say so.
    pub fn expand_home_with(&self, home: &str) -> Capability {
        let home = home.to_string();
        let scope = if self.scope == "~" {
            home
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

    /// Does this capability (as *granted*) cover `req` (as *requested*)?
    ///
    /// Grants may use `*` as a wildcard for domain or action, and glob syntax in
    /// scope. Requests are always concrete.
    pub fn covers(&self, req: &Capability) -> bool {
        wild_eq(&self.domain, &req.domain)
            && wild_eq(&self.action, &req.action)
            && glob_match(&self.scope, &req.scope)
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

/// Every capability the system knows how to adjudicate and execute.
///
/// A name absent from this list is not merely unusual — it is unimplementable,
/// and the GLYPH checker rejects it before anything runs. Keeping the list
/// explicit is what makes "what can this program possibly do?" an answerable
/// question.
pub const KNOWN_CAPABILITIES: &[&str] = &[
    "fs.read",
    "fs.stat",
    "fs.list",
    "fs.search",
    "fs.write",
    "fs.mkdir",
    "fs.move",
    "fs.delete",
    "fs.chmod",
    "fs.chown",
    "proc.list",
    "proc.signal",
    "proc.spawn",
    "sys.info",
    "sys.metrics",
    "sys.power",
    "sys.mount",
    "sys.firmware",
    "svc.status",
    "svc.start",
    "svc.stop",
    "svc.restart",
    "pkg.query",
    "pkg.install",
    "pkg.remove",
    "net.status",
    "net.connect",
    "net.listen",
    "shell.exec",
    "model.infer",
    "ctx.read",
    "ctx.write",
    "journal.revert",
    "ui.notify",
    "ui.render",
    "journal.read",
    "media.probe",
    "media.search",
    "media.thumbnail",
    "media.play",
    "media.control",
    "media.edit",
    "media.render",
    "media.index",
    "curate.scan",
    "curate.propose",
    "curate.apply",
    "desk.apps",
    "desk.windows",
    "desk.session_info",
    "desk.notify",
    "desk.copy",
    "desk.clipboard",
    "desk.focus",
    "desk.open",
    "desk.launch",
    "desk.close",
    "desk.screenshot",
    "desk.setting",
    "desk.session",
    "policy.amend",
    "secret.read",
    "user.admin",
];

/// Is `name` (a bare `domain.action`) something the system implements?
pub fn is_known(name: &str) -> bool {
    KNOWN_CAPABILITIES.contains(&name)
}

fn wild_eq(pattern: &str, actual: &str) -> bool {
    pattern == "*" || pattern == actual
}

/// Glob matcher supporting `?`, `*` (one path segment) and `**` (any depth).
///
/// Written iteratively with backtracking rather than recursively so that a
/// pathological pattern from a config file cannot blow the daemon's stack.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" || pattern == "**" || pattern == text {
        return true;
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();

    let (mut pi, mut ti) = (0usize, 0usize);
    // Saved backtrack point for the most recent `*` / `**`.
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
            // `**/` should also match zero segments, so swallow a following slash.
            if double && pi < p.len() && p[pi] == '/' {
                pi += 1;
            }
            star_p = Some(pi);
            star_t = ti;
        } else if let Some(sp) = star_p {
            // Backtrack: let the star consume one more character.
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

    // The text is exhausted. What is left of the pattern must be able to match
    // nothing at all -- which is true of trailing stars, and of a trailing
    // `/**` (so `/var/**` covers `/var` itself, not just its contents).
    let rest: String = p[pi..].iter().collect();
    rest.is_empty() || rest.chars().all(|c| c == '*') || rest == "/**"
}

/// Paths the system refuses to let *any* subject write, whatever policy says.
///
/// This is the immutable floor. Policy files can widen what agents may do, but
/// they cannot reach through this list — an AI that can rewrite the bootloader
/// or the policy engine's own rules is one bad inference away from an unbootable
/// machine, and no amount of confidence in the model justifies that.
pub const PROTECTED_WRITE_PATHS: &[&str] = &[
    "/boot/**",
    "/sys/firmware/**",
    "/dev/sd*",
    "/dev/nvme*",
    "/dev/mapper/**",
    "/etc/shadow",
    "/etc/gshadow",
    "/etc/sudoers",
    "/etc/sudoers.d/**",
    "/etc/nous/policy.d/**",
    "/proc/sys/kernel/**",
    "/usr/lib/modules/**",
];

/// Paths that may never be *read* into model context, however the request is
/// phrased. Anything matching here is a secret whose value has no business
/// crossing an inference boundary.
pub const PROTECTED_READ_PATHS: &[&str] = &[
    "/etc/shadow",
    "/etc/gshadow",
    "/etc/ssh/*_key",
    "/**/.ssh/id_*",
    "/**/.gnupg/**",
    "/**/.aws/credentials",
    "/**/.config/nous/secrets/**",
    "/**/*.pem",
    "/**/*.key",
];

/// Returns the pattern that forbids this request, if any.
pub fn protected_violation(cap: &Capability) -> Option<&'static str> {
    let list = match (cap.domain.as_str(), cap.action.as_str()) {
        ("fs", "write") | ("fs", "delete") | ("fs", "move") | ("fs", "chmod") | ("fs", "chown") => {
            PROTECTED_WRITE_PATHS
        }
        ("fs", "read") => PROTECTED_READ_PATHS,
        _ => return None,
    };
    list.iter().find(|p| glob_match(p, &cap.scope)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_capability_forms() {
        let c = Capability::parse("fs.write:/home/**").unwrap();
        assert_eq!(c.domain, "fs");
        assert_eq!(c.action, "write");
        assert_eq!(c.scope, "/home/**");

        let bare = Capability::parse("proc.list").unwrap();
        assert_eq!(bare.scope, "*");
        assert_eq!(bare.to_string(), "proc.list");
    }

    #[test]
    fn rejects_malformed_capabilities() {
        assert!(Capability::parse("fs").is_err());
        assert!(Capability::parse("").is_err());
        assert!(Capability::parse(".write").is_err());
    }

    #[test]
    fn every_known_capability_has_a_deliberate_risk() {
        // A name in the known list that still falls through to the catch-all is
        // a table the author forgot to extend.
        for name in KNOWN_CAPABILITIES {
            let c = Capability::parse(name).unwrap();
            let deliberate = matches!(
                (c.domain.as_str(), c.action.as_str()),
                ("policy", "amend")
                    | ("secret", "read")
                    | ("user", "admin")
                    | ("sys", "power")
                    | ("sys", "mount")
                    | ("sys", "firmware")
                    | ("desk", "session")
            );
            assert!(
                c.risk() != Risk::Critical || deliberate,
                "{} falls through to the unknown-capability catch-all",
                name
            );
        }
    }

    #[test]
    fn unknown_capabilities_are_critical() {
        assert_eq!(
            Capability::parse("wat.dothis").unwrap().risk(),
            Risk::Critical
        );
        assert_eq!(Capability::parse("fs.read").unwrap().risk(), Risk::Read);
        assert_eq!(
            Capability::parse("fs.delete").unwrap().risk(),
            Risk::Elevated
        );
    }

    #[test]
    fn single_star_does_not_cross_separators() {
        assert!(glob_match("/home/*", "/home/joey"));
        assert!(!glob_match("/home/*", "/home/joey/notes"));
        assert!(glob_match("/home/**", "/home/joey/notes/deep/file.txt"));
    }

    #[test]
    fn double_star_matches_zero_segments() {
        assert!(glob_match("/var/**", "/var"));
        assert!(glob_match("/**/.ssh/id_*", "/home/joey/.ssh/id_rsa"));
        assert!(!glob_match("/**/.ssh/id_*", "/home/joey/notes.txt"));
    }

    #[test]
    fn grants_cover_narrower_requests() {
        let grant = Capability::parse("fs.read:/home/joey/**").unwrap();
        assert!(grant.covers(&Capability::parse("fs.read:/home/joey/a/b.txt").unwrap()));
        assert!(!grant.covers(&Capability::parse("fs.read:/etc/passwd").unwrap()));
        assert!(!grant.covers(&Capability::parse("fs.write:/home/joey/a").unwrap()));

        let wide = Capability::parse("fs.*:/tmp/**").unwrap();
        assert!(wide.covers(&Capability::parse("fs.delete:/tmp/x").unwrap()));
    }

    #[test]
    fn home_relative_scopes_expand_for_both_sides() {
        // An explicit home rather than the environment: these tests run in
        // parallel in one process, and HOME is shared state.
        let home = "/export/home/joey";
        let grant = Capability::parse("fs.write:~/**")
            .unwrap()
            .expand_home_with(home);
        assert_eq!(grant.scope, "/export/home/joey/**");

        let request = Capability::parse("fs.write:~/notes.md")
            .unwrap()
            .expand_home_with(home);
        assert_eq!(request.scope, "/export/home/joey/notes.md");
        assert!(grant.covers(&request));

        // An absolute request under the same home is covered too.
        let absolute = Capability::parse("fs.write:/export/home/joey/a/b.txt").unwrap();
        assert!(grant.covers(&absolute.expand_home_with(home)));

        // Someone else's home is not.
        assert!(!grant.covers(&Capability::parse("fs.write:/export/home/other/x").unwrap()));
    }

    #[test]
    fn expanding_leaves_absolute_scopes_alone() {
        let c = Capability::parse("fs.read:/etc/hosts").unwrap();
        assert_eq!(c.expand_home_with("/home/joey").scope, "/etc/hosts");
        assert_eq!(
            Capability::parse("proc.list")
                .unwrap()
                .expand_home_with("/home/joey")
                .scope,
            "*"
        );
    }

    #[test]
    fn protected_paths_are_detected() {
        let boot = Capability::parse("fs.write:/boot/grub/grub.cfg").unwrap();
        assert_eq!(protected_violation(&boot), Some("/boot/**"));

        let key = Capability::parse("fs.read:/home/joey/.ssh/id_ed25519").unwrap();
        assert!(protected_violation(&key).is_some());

        let ok = Capability::parse("fs.write:/home/joey/notes.md").unwrap();
        assert_eq!(protected_violation(&ok), None);

        // Reading /boot is fine; writing it is not.
        assert_eq!(
            protected_violation(&Capability::parse("fs.read:/boot/x").unwrap()),
            None
        );
    }
}
