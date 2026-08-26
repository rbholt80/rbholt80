//! Render a folder to PNG, with thumbnails, so the file view can be looked at.
//!
//! `cargo run -p nous-ui --example folder -- <outdir>`

use nous_ui::draw::{Image, Rgba};
use nous_ui::files::{Entry, Files, Layout, Mark};
use nous_ui::theme::{Risk, Theme};

/// Stand-in photographs. The real ones are PNG thumbnails cached by the daemon;
/// these are generated so the example needs no media on disk.
fn make_thumb(dir: &str, name: &str, a: Rgba, b: Rgba) -> String {
    let img = Image::new(240, 160).expect("thumb surface");
    let c = img.canvas();
    for y in 0..160 {
        let t = y as f64 / 159.0;
        c.fill_rect(
            nous_ui::draw::Rect::new(0.0, y as f64, 240.0, 1.0),
            a.mix(b, t),
        );
    }
    let path = format!("{dir}/{name}");
    img.write_png(&path).expect("wrote thumb");
    path
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let _ = std::fs::create_dir_all(&dir);
    let thumbs = format!("{dir}/thumbs");
    let _ = std::fs::create_dir_all(&thumbs);

    let palette = [
        (Rgba::rgb(64, 92, 140), Rgba::rgb(150, 176, 200)),
        (Rgba::rgb(140, 96, 64), Rgba::rgb(206, 170, 130)),
        (Rgba::rgb(72, 116, 88), Rgba::rgb(158, 196, 150)),
        (Rgba::rgb(112, 76, 120), Rgba::rgb(186, 158, 196)),
        (Rgba::rgb(140, 120, 60), Rgba::rgb(214, 200, 140)),
    ];

    let mut entries: Vec<Entry> = Vec::new();
    let mut push = |e: Entry| entries.push(e);

    push(Entry {
        name: "Pictures".into(),
        path: "/home/joey/Pictures".into(),
        is_dir: true,
        size: 0,
        thumb: None,
        mark: None,
    });
    for i in 1..=6 {
        let (a, b) = palette[(i - 1) % palette.len()];
        push(Entry {
            name: format!("holiday-{i}.jpg"),
            path: format!("/home/joey/Downloads/holiday-{i}.jpg"),
            is_dir: false,
            size: 2_400_000 + i as u64 * 130_000,
            thumb: Some(make_thumb(&thumbs, &format!("h{i}.png"), a, b)),
            mark: None,
        });
    }
    let (a, b) = palette[0];
    push(Entry {
        name: "holiday-1-copy.jpg".into(),
        path: "/home/joey/Downloads/holiday-1-copy.jpg".into(),
        is_dir: false,
        size: 2_400_000,
        thumb: Some(make_thumb(&thumbs, "dupe.png", a, b)),
        mark: Some(Mark {
            risk: Risk::Elevated,
            note: "same picture as holiday-1.jpg".into(),
        }),
    });
    push(Entry {
        name: "clip.mp4".into(),
        path: "/home/joey/Downloads/clip.mp4".into(),
        is_dir: false,
        size: 48_200_000,
        thumb: Some(make_thumb(&thumbs, "clip.png", palette[2].0, palette[2].1)),
        mark: Some(Mark {
            risk: Risk::Write,
            note: "a video, filed with photos".into(),
        }),
    });
    push(Entry {
        name: "invoice-2024.pdf".into(),
        path: "/home/joey/Downloads/invoice-2024.pdf".into(),
        is_dir: false,
        size: 184_000,
        thumb: None,
        mark: None,
    });
    push(Entry {
        name: "installer.run".into(),
        path: "/home/joey/Downloads/installer.run".into(),
        is_dir: false,
        size: 1_900_000_000,
        thumb: None,
        mark: Some(Mark {
            risk: Risk::Critical,
            note: "1.8 GB, never opened".into(),
        }),
    });
    push(Entry {
        name: "notes.txt".into(),
        path: "/home/joey/Downloads/notes.txt".into(),
        is_dir: false,
        size: 3_100,
        thumb: None,
        mark: None,
    });

    for (suffix, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
        let mut f = Files::new("/home/joey/Downloads", entries.clone());
        f.selected = 7;
        f.proposal = Some("move 1 video, remove 1 duplicate, free 1.8 GB".into());

        let (w, h) = (940.0, 620.0);
        let img = Image::new(w as i32, h as i32).expect("surface");
        let layout = Layout::compute(&f, w, h);
        nous_ui::files::render(&img.canvas(), &mut f, &theme, &layout);
        let path = format!("{dir}/files-{suffix}.png");
        match img.write_png(&path) {
            Ok(()) => println!(
                "{path}  {}x{}  {} columns",
                img.width, img.height, layout.columns
            ),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
}
