//! Render a folder as a weighted field, so the arrangement can be looked at.
//!
//! `cargo run -p nous-ui --example field -- <outdir>`

use nous_ui::draw::Image;
use nous_ui::field::{render, Field};
use nous_ui::files::{Entry, Mark};
use nous_ui::theme::{Risk, Theme};

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `days` is how long ago it was last touched, which is what decides how much
/// room it gets.
fn e(name: &str, size: u64, days: u64) -> Entry {
    Entry {
        name: name.into(),
        path: format!("/home/joey/Downloads/{name}"),
        is_dir: false,
        size,
        modified: now().saturating_sub(days * 86400),
        thumb: None,
        blurb: None,
        mark: None,
    }
}

fn dir(name: &str) -> Entry {
    Entry {
        is_dir: true,
        ..e(name, 0, 1)
    }
}

fn mark(mut x: Entry, risk: Risk, note: &str) -> Entry {
    x.mark = Some(Mark {
        risk,
        note: note.into(),
    });
    x
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let _ = std::fs::create_dir_all(&out);

    // A real-shaped Downloads folder: a handful that matter, a long tail that
    // does not.
    let mut entries = vec![dir("Invoices"), dir("Photos 2026")];
    // This week's work.
    entries.push(e("report-2026.pdf", 2_400_000, 0));
    entries.push(e("2026-budget.xlsx", 180_000, 1));
    entries.push(e("interview.mp4", 1_200_000_000, 2));
    entries.push(e("holiday.jpg", 4_800_000, 3));
    entries.push(e("notes.md", 3_100, 0));
    // Things the curator has an opinion about, whatever their age.
    entries.push(mark(
        e("installer.run", 1_900_000_000, 330),
        Risk::Critical,
        "1.8 GB, never opened in 11 months",
    ));
    entries.push(mark(
        e("archive.zip", 240_000_000, 40),
        Risk::Elevated,
        "same file as report-2026.pdf",
    ));
    entries.push(mark(
        e("talk.mp4", 480_000_000, 12),
        Risk::Write,
        "a video, filed with documents",
    ));
    // Last month.
    entries.push(e("sixteen-tons.flac", 38_000_000, 26));
    entries.push(e("blue-monk.flac", 62_000_000, 34));
    for i in 0..9 {
        entries.push(e(
            &format!("scan-{i:02}.jpg"),
            900_000 + i * 40_000,
            18 + i * 3,
        ));
    }
    // The long tail nobody has touched in a year.
    for i in 0..14 {
        entries.push(e(
            &format!("old-receipt-{i:02}.pdf"),
            40_000 + i * 900,
            300 + i * 4,
        ));
    }
    for i in 0..7 {
        entries.push(e(
            &format!("backup-{i}.tar.gz"),
            300_000_000 + i * 1_000_000,
            210 + i * 9,
        ));
    }

    let now = now();

    for (suffix, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
        let (w, h) = (1100.0, 660.0);
        let area = nous_ui::draw::Rect::new(0.0, 0.0, w, h);
        let field = Field::arrange(&entries, area, now);
        let img = Image::new(w as i32, h as i32).expect("surface");
        render(
            &img.canvas(),
            &field,
            &entries,
            &[2, 3],
            &theme,
            area,
            &mut nous_ui::field::Pictures::default(),
            None,
        );
        let path = format!("{out}/field-{suffix}.png");
        match img.write_png(&path) {
            Ok(()) => println!(
                "{path}  {} files in {} families",
                entries.len(),
                field.clusters.len()
            ),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
}
