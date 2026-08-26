//! Render the media player to PNG so the editing surface can be looked at.
//!
//! `cargo run -p nous-ui --example player -- <outdir>`

use nous_ui::draw::{Image, Rect, Rgba};
use nous_ui::player::{Clip, Layout, Player, Transport};
use nous_ui::theme::Theme;

/// Stand-in frames. The real ones are PNG thumbnails cached by the daemon;
/// these are generated so the example needs no media on disk.
fn make_frame(dir: &str, name: &str, a: Rgba, b: Rgba) -> String {
    let img = Image::new(320, 180).expect("frame surface");
    let c = img.canvas();
    for y in 0..180 {
        let t = y as f64 / 179.0;
        c.fill_rect(Rect::new(0.0, y as f64, 320.0, 1.0), a.mix(b, t));
    }
    let path = format!("{dir}/{name}");
    img.write_png(&path).expect("wrote frame");
    path
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let _ = std::fs::create_dir_all(&dir);
    let frames = format!("{dir}/frames");
    let _ = std::fs::create_dir_all(&frames);

    let palette = [
        (Rgba::rgb(46, 78, 122), Rgba::rgb(158, 190, 214)),
        (Rgba::rgb(126, 84, 56), Rgba::rgb(212, 174, 132)),
        (Rgba::rgb(58, 104, 78), Rgba::rgb(150, 194, 148)),
        (Rgba::rgb(96, 64, 106), Rgba::rgb(178, 148, 190)),
    ];

    let names = [
        ("arrival.mp4", 42.0, 0.0, 38.5, 1.0),
        ("interview.mp4", 300.0, 12.0, 96.0, 1.0),
        ("b-roll.mp4", 60.0, 4.0, 34.0, 1.5),
        ("closing.mp4", 25.0, 0.0, 25.0, 1.0),
    ];

    let clips: Vec<Clip> = names
        .iter()
        .enumerate()
        .map(|(i, (name, src, start, end, speed))| {
            let (a, b) = palette[i % palette.len()];
            Clip {
                id: format!("c{}", i + 1),
                path: format!("/home/joey/Videos/{name}"),
                name: (*name).to_string(),
                start: *start,
                end: *end,
                source_duration: *src,
                speed: *speed,
                volume: 1.0,
                thumb: Some(make_frame(&frames, &format!("f{i}.png"), a, b)),
            }
        })
        .collect();

    for (suffix, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
        let mut p = Player::new("holiday-cut", clips.clone());
        p.select(1);
        p.seek(p.clip_start(1) + 22.0);
        p.transport = Transport::Playing;

        let (w, h) = (940.0, 620.0);
        let img = Image::new(w as i32, h as i32).expect("surface");
        let layout = Layout::compute(&p, w, h);
        nous_ui::player::render(&img.canvas(), &mut p, &theme, &layout);
        let path = format!("{dir}/player-{suffix}.png");
        match img.write_png(&path) {
            Ok(()) => println!(
                "{path}  {}x{}  {:.1}s timeline",
                img.width,
                img.height,
                p.duration()
            ),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
}
