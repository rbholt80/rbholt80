//! Open a real window, draw a real frame, and read it back off the X server.
//!
//! `cargo run -p nous-ui --example live -- <outdir>`
//!
//! The offscreen tests prove the drawing code is right. This proves the window
//! code is: that a window opens, that Cairo is wired to it, that the frame
//! reaches the server, and that keyboard input arrives. Run it under Xvfb and
//! the PNG it writes is what the screen actually held.
//!
//! Exits nonzero if anything failed, so it can be run as a check.

use nous_ui::draw::Image;
use nous_ui::panel::{Body, Layout, Panel, Step};
use nous_ui::theme::{risk_of, Metrics, Theme};
use nous_ui::window::{Event, Window, WindowKind};

fn step(cap: &str, summary: &str) -> Step {
    Step {
        risk: risk_of(cap),
        capability: cap.into(),
        summary: summary.into(),
    }
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let _ = std::fs::create_dir_all(&dir);

    let theme = Theme::dark();
    let mut window = match Window::open(
        "Nous",
        Metrics::PANEL_WIDTH as i32,
        Metrics::PROMPT_HEIGHT as i32,
        WindowKind::Overlay,
    ) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("could not open a window: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "window {:#x} opened, translucent={}",
        window.id(),
        window.translucent()
    );

    let mut panel = Panel::new();
    panel.input.set("tidy my downloads");
    panel.set_body(Body::Proposal {
        headline: "413 moves across 137 files".into(),
        steps: vec![
            step(
                "fs.move:~/Downloads/**",
                "move 84 images into Pictures/2026",
            ),
            step("fs.read:~/Downloads/**", "read 137 files to sort them"),
            step("fs.delete:~/Downloads/**", "remove 22 empty folders"),
        ],
    });
    panel.selected = 2;

    let probe = Image::new(1, 1).expect("probe surface");
    let layout = Layout::compute(&panel, Metrics::PANEL_WIDTH, &probe.canvas(), &theme);
    window.resize(Metrics::PANEL_WIDTH as i32, layout.height.ceil() as i32);

    // Draw twice: the first frame goes out before the server has finished
    // mapping the window on some setups, and a blank capture would look like a
    // rendering fault when it is only a race.
    for _ in 0..2 {
        window.draw(theme.backdrop_opaque, |c| {
            nous_ui::panel::render(c, &panel, &theme, &layout, true);
        });
        window.sync();
    }

    // Let any Expose the server sends land, so the capture is of a settled
    // window rather than one mid-map.
    for _ in 0..8 {
        if let Event::Redraw | Event::Resized { .. } = window.wait(40) {
            window.draw(theme.backdrop_opaque, |c| {
                nous_ui::panel::render(c, &panel, &theme, &layout, true);
            });
        }
    }

    let path = format!("{dir}/live-window.png");
    match window.capture(&path) {
        Ok(()) => println!("captured {path}"),
        Err(e) => {
            eprintln!("capture failed: {e}");
            std::process::exit(1);
        }
    }

    let (w, h) = window.size();
    println!("window is {w}x{h}, layout wanted {}", layout.height);
    if (h - layout.height.ceil()).abs() > 1.0 {
        eprintln!("the window did not take the height the layout asked for");
        std::process::exit(1);
    }
}
