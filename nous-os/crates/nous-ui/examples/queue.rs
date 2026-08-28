//! Render the play queue to PNG so the player can be looked at.
//!
//! `cargo run -p nous-ui --example queue -- <outdir>`

use nous_ui::draw::{Image, Rect, Rgba};
use nous_ui::queue::{Kind, Layout, Queue, Stream, Track};
use nous_ui::theme::Theme;

fn make_art(dir: &str, name: &str, a: Rgba, b: Rgba) -> String {
    let img = Image::new(300, 300).expect("art surface");
    let c = img.canvas();
    for y in 0..300 {
        let t = y as f64 / 299.0;
        c.fill_rect(Rect::new(0.0, y as f64, 300.0, 1.0), a.mix(b, t));
    }
    // A band across it, so the crop is visible rather than merely a wash.
    c.fill_rect(Rect::new(0.0, 190.0, 300.0, 26.0), b.mix(a, 0.25));
    let path = format!("{dir}/{name}");
    img.write_png(&path).expect("wrote art");
    path
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let _ = std::fs::create_dir_all(&dir);
    let art_dir = format!("{dir}/art");
    let _ = std::fs::create_dir_all(&art_dir);

    let cover = make_art(
        &art_dir,
        "cover.png",
        Rgba::rgb(38, 54, 92),
        Rgba::rgb(206, 148, 96),
    );

    let names = [
        ("Sixteen Tons", "Tennessee Ernie Ford", 168.0),
        ("Blue Monk", "Thelonious Monk", 512.0),
        ("Alabama", "John Coltrane", 303.0),
        ("Peace Piece", "Bill Evans", 402.0),
        ("So What", "Miles Davis", 562.0),
        ("Take Five", "Dave Brubeck", 324.0),
        ("Naima", "John Coltrane", 267.0),
        ("Round Midnight", "Miles Davis", 351.0),
    ];

    let tracks: Vec<Track> = names
        .iter()
        .enumerate()
        .map(|(i, (title, artist, secs))| Track {
            path: format!(
                "/home/joey/Music/{}.flac",
                title.to_lowercase().replace(' ', "-")
            ),
            title: (*title).to_string(),
            artist: (*artist).to_string(),
            duration: *secs,
            kind: Kind::Audio,
            art: if i == 1 { Some(cover.clone()) } else { None },
        })
        .collect();

    for (suffix, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
        // A music queue: the stage has to carry the cover and the name.
        let mut q = Queue::new(tracks.clone());
        q.current = Some(1);
        q.selected = 4;
        q.position = 137.0;
        q.duration = 512.0;
        q.volume = 72.0;
        q.speed = 1.0;

        let (w, h) = (1100.0, 640.0);
        let img = Image::new(w as i32, h as i32).expect("surface");
        let layout = Layout::compute(&q, w, h);
        nous_ui::queue::render(&img.canvas(), &mut q, &theme, &layout);
        let path = format!("{dir}/queue-{suffix}.png");
        match img.write_png(&path) {
            Ok(()) => println!("{path}  {}x{}", img.width, img.height),
            Err(e) => eprintln!("{path}: {e}"),
        }

        // A film: the picture belongs to the player, and the file offers
        // alternate audio and subtitles.
        let mut f = Queue::new(vec![Track {
            path: "/home/joey/Videos/la-jetee.mkv".into(),
            title: "La Jetée".into(),
            artist: String::new(),
            duration: 1680.0,
            kind: Kind::Video,
            art: None,
        }]);
        f.current = Some(0);
        f.position = 622.0;
        f.duration = 1680.0;
        f.volume = 88.0;
        f.speed = 1.25;
        f.audio = vec![
            Stream {
                id: "1".into(),
                label: "français".into(),
                selected: true,
            },
            Stream {
                id: "2".into(),
                label: "Commentary (eng)".into(),
                selected: false,
            },
        ];
        f.subtitles = vec![
            Stream {
                id: "1".into(),
                label: "English".into(),
                selected: true,
            },
            Stream {
                id: "2".into(),
                label: "français".into(),
                selected: false,
            },
        ];
        let img = Image::new(w as i32, h as i32).expect("surface");
        let layout = Layout::compute(&f, w, h);
        nous_ui::queue::render(&img.canvas(), &mut f, &theme, &layout);
        let path = format!("{dir}/film-{suffix}.png");
        match img.write_png(&path) {
            Ok(()) => println!("{path}  {}x{}", img.width, img.height),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
}
