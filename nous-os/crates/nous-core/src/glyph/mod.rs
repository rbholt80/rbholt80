//! # GLYPH — the NOUS intent language
//!
//! GLYPH exists because of one problem: a system that lets a model act on your
//! machine needs an answer to "what is about to happen?" that is *complete* and
//! available *before* anything happens. Ordinary code cannot give you that —
//! you cannot know what a shell script will touch without running it.
//!
//! So GLYPH is not a scripting language. Every statement in it is a capability
//! request, which means a flow can be read statically and turned into an exact
//! list of everything it may do ([`check::Manifest`]), adjudicated against
//! policy, and shown to a human — all before the first step executes.
//!
//! ```text
//! flow tidy-downloads {
//!   meta description "Move stray media out of Downloads"
//!
//!   found = curate.scan    roots: [~/Downloads]
//!   plan  = curate.propose kinds: [misfiled_media, duplicate]
//!
//!   gate plan.count > 0
//!   ask  "Move ${plan.count} files?"
//!
//!   curate.apply steps: plan.steps
//! }
//! ```
//!
//! Three properties fall out of that shape:
//!
//! - **Auditable.** `nousctl glyph check` prints the full capability manifest.
//! - **Reversible.** Each step is journalled with its own undo, so a flow can
//!   be unwound after the fact.
//! - **Portable.** Capabilities are abstract; the executors underneath them are
//!   platform-specific, and `on linux { ... }` / `on windows { ... }` blocks
//!   handle what genuinely differs. Software that predates GLYPH is reached
//!   through `use foreign`, which compiles to a governed `shell.exec` rather
//!   than an escape hatch out of the model.

pub mod ast;
pub mod check;
pub mod lex;
pub mod parse;

pub use ast::{current_platform, Call, Cond, Flow, Foreign, Program, Stmt, Value};
pub use check::{check, Diagnostic, Manifest, Requested, Severity};
pub use lex::Piece;
pub use parse::parse;

/// Flatten string pieces to a literal, or `None` if any piece interpolates.
pub fn render_literal(pieces: &[Piece]) -> Option<String> {
    let mut out = String::new();
    for p in pieces {
        match p {
            Piece::Lit(s) => out.push_str(s),
            Piece::Ref(_) => return None,
        }
    }
    Some(out)
}

/// Parse and check a whole document, returning one manifest per flow.
pub fn lint(src: &str, platform: &str) -> Result<Vec<Manifest>, String> {
    let program = parse(src)?;
    Ok(program.flows.iter().map(|f| check(f, platform)).collect())
}

/// The conventional file extension for GLYPH programs.
pub const EXTENSION: &str = "glyph";

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
flow tidy-downloads {
  meta description "Move stray media out of Downloads"
  meta author "nous"

  found = curate.scan    roots: [~/Downloads]
  plan  = curate.propose kinds: [misfiled_media, duplicate]

  gate plan.count > 0
  ask  "Move ${plan.count} files?"

  curate.apply steps: plan.steps
}
"#;

    #[test]
    fn the_documented_example_parses_and_checks_clean() {
        let manifests = lint(EXAMPLE, "linux").unwrap();
        assert_eq!(manifests.len(), 1);
        let m = &manifests[0];
        assert!(m.is_valid(), "{:?}", m.diagnostics);
        assert_eq!(m.flow, "tidy-downloads");
        assert_eq!(m.description, "Move stray media out of Downloads");
        assert_eq!(m.requests.len(), 3);
        assert_eq!(m.asks, 1);
        assert_eq!(m.gates, 1);
    }

    #[test]
    fn a_document_may_hold_several_flows() {
        let src = "flow a { sys.info }\nflow b { sys.metrics }";
        let ms = lint(src, "linux").unwrap();
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[1].flow, "b");
        assert_eq!(parse(src).unwrap().flow("b").unwrap().stmts.len(), 1);
    }

    #[test]
    fn syntax_errors_surface_before_any_checking() {
        let err = lint("flow a { fs.list path: }", "linux").unwrap_err();
        assert!(err.contains("expected a value"), "{err}");
    }

    #[test]
    fn render_literal_refuses_interpolated_pieces() {
        assert_eq!(
            render_literal(&[Piece::Lit("abc".into())]),
            Some("abc".to_string())
        );
        assert_eq!(render_literal(&[Piece::Ref("x.y".into())]), None);
    }
}
