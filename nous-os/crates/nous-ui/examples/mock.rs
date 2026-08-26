//! Render the panel's states to PNG files so the design can be looked at.
//!
//! `cargo run -p nous-ui --example mock -- <outdir>`
//!
//! No X server involved: this draws to the same offscreen surface the tests
//! use, which means what it shows is what the window will show.

use nous_ui::draw::Image;
use nous_ui::panel::{Body, Layout, Panel, Step};
use nous_ui::theme::{Metrics, Risk, Theme};

fn step(cap: &str, summary: &str, risk: Risk) -> Step {
    Step {
        capability: cap.into(),
        summary: summary.into(),
        risk,
    }
}

fn shot(dir: &str, name: &str, theme: &Theme, panel: &Panel) {
    let w = Metrics::PANEL_WIDTH;
    // Two passes: measure with a throwaway surface to find the height, then
    // draw at exactly that height so the PNG has no dead space.
    let probe = Image::new(1, 1).expect("probe surface");
    let layout = Layout::compute(panel, w, &probe.canvas(), theme);

    let img = Image::new(w as i32, layout.height.ceil() as i32).expect("surface");
    let c = img.canvas();
    nous_ui::panel::render(&c, panel, theme, &layout, true);
    let path = format!("{dir}/{name}.png");
    match img.write_png(&path) {
        Ok(()) => println!("{path}  {}x{}", img.width, img.height),
        Err(e) => eprintln!("{path}: {e}"),
    }
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let _ = std::fs::create_dir_all(&dir);

    for (suffix, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
        let mut p = Panel::new();
        shot(&dir, &format!("01-empty-{suffix}"), &theme, &p);

        p.input.set("tidy my downloads");
        shot(&dir, &format!("02-typing-{suffix}"), &theme, &p);

        p.set_body(Body::Working {
            note: "looking through 137 files…".into(),
        });
        p.phase = 1.2;
        shot(&dir, &format!("03-working-{suffix}"), &theme, &p);

        p.input.set("claude what is a capability");
        p.set_body(Body::Answer {
            source: "claude".into(),
            text: "A capability is a permission written as domain.action:scope — \
                   fs.move:~/Downloads/** is the right to move files under that \
                   folder and nowhere else. Every action the system takes names \
                   one, and the policy decides before anything runs."
                .into(),
        });
        shot(&dir, &format!("04-answer-{suffix}"), &theme, &p);

        p.input.set("tidy my downloads");
        p.set_body(Body::Proposal {
            headline: "413 moves across 137 files".into(),
            steps: vec![
                step(
                    "fs.move:~/Downloads/**",
                    "move 84 images into Pictures/2026",
                    Risk::Write,
                ),
                step(
                    "fs.move:~/Downloads/**",
                    "move 31 documents into Documents",
                    Risk::Write,
                ),
                step(
                    "fs.read:~/Downloads/**",
                    "read 137 files to sort them",
                    Risk::Read,
                ),
                step(
                    "fs.delete:~/Downloads/**",
                    "remove 22 empty folders",
                    Risk::Critical,
                ),
                step(
                    "fs.move:~/Downloads/**",
                    "move 12 archives into Archives",
                    Risk::Write,
                ),
                step("fs.index:~/**", "reindex the moved files", Risk::Read),
            ],
        });
        p.selected = 3;
        shot(&dir, &format!("05-proposal-{suffix}"), &theme, &p);

        p.set_body(Body::Done {
            headline: "moved 137 files. nothing was deleted.".into(),
            detail: "84 images into Pictures/2026\n31 documents into Documents\n22 archives into Archives".into(),
            undo_hint: true,
        });
        shot(&dir, &format!("06-done-{suffix}"), &theme, &p);

        p.set_body(Body::Error {
            message: "no model reachable — set a key with :key claude sk-…".into(),
        });
        shot(&dir, &format!("07-error-{suffix}"), &theme, &p);
    }
}
