//! `nous` — the window.
//!
//! Everything the interface can draw was, until this existed, reachable only
//! from a test harness that wrote PNG files. The surfaces were real and the
//! pictures of them were honest, but nothing opened one. This opens one.
//!
//! A single resizable application window holding the views, switched between
//! rather than stacked: files, the player, and the cutting room. The panel —
//! the thing you talk to — stays a separate overlay summoned by a hotkey,
//! because it is summoned over whatever you are already doing and a view inside
//! this window could not be.

mod ask;
mod check;
mod curated;
mod filepane;
mod find;
mod history;
mod link;
mod manage;
mod places;
mod viewer;
mod views;

use nous_ui::draw::Image;
use nous_ui::ffi;
use nous_ui::theme::Theme;
use nous_ui::window::{Event, Window, WindowKind};
use views::{App, View};

const WIDTH: i32 = 1180;
const HEIGHT: i32 = 720;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", HELP);
        return;
    }
    // Say what the window can and cannot reach, and why. Needs no display, so
    // it works from a terminal over ssh and on a machine where the window
    // itself will not open.
    if args.iter().any(|a| a == "--check") {
        std::process::exit(check::run());
    }
    // A screenshot of the real window, for checking the thing that opens rather
    // than an offscreen copy of it. Used by the self-test.
    let shot = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1).cloned());
    let start = args
        .iter()
        .position(|a| a == "--view")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| View::named(n));
    // Open on a particular folder. What a file manager's "open in..." needs,
    // and what anyone typing `nous ~/Pictures` expects — so a bare path works
    // as well as the flag.
    let folder = args
        .iter()
        .position(|a| a == "--folder")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .or_else(|| {
            args.iter()
                .find(|a| !a.starts_with('-') && std::path::Path::new(a).is_dir())
                .cloned()
        });

    // Detected from the desktop's own setting, and overridable — detection
    // reads a GTK preference that not every desktop sets, and being stuck in
    // the wrong theme with no way to say so is a poor first impression.
    let theme = match args
        .iter()
        .position(|a| a == "--theme")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
    {
        Some("dark") => Theme::dark(),
        Some("light") => Theme::light(),
        Some(other) => {
            eprintln!("nous: unknown theme '{other}' — use dark or light");
            std::process::exit(2);
        }
        None => Theme::detect(),
    };
    let mut window = match Window::open("Nous", WIDTH, HEIGHT, WindowKind::Normal) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("nous: could not open a window: {e}");
            eprintln!("      is DISPLAY set? this needs a running X session.");
            std::process::exit(1);
        }
    };

    let (mut w, mut h) = (WIDTH as f64, HEIGHT as f64);
    let mut app = App::new(start.unwrap_or(View::Files));
    if let Some(f) = &folder {
        app.pane.go(std::path::PathBuf::from(f));
    }
    app.load();

    // Draw twice before anything is read back: the first frame can go out
    // before the server has finished mapping the window, and a blank capture
    // would look like a rendering fault when it is only a race.
    app.settle();
    for _ in 0..2 {
        window.draw(theme.backdrop_opaque, |c| app.render(c, &theme, w, h));
        window.sync();
    }

    // A menu cannot be opened by a screenshot run, which has no pointer. This
    // opens one so the menu can be looked at like anything else.
    if args.iter().any(|a| a == "--with-menu") {
        app.demo_menu(w, h);
    }
    // Curator marks, without a daemon to produce them, so the drawing of them
    // can be looked at. Says so on screen: an interface that showed invented
    // findings without saying they were invented would be lying.
    if let Some(i) = args.iter().position(|a| a == "--demo-marks") {
        app.demo_marks(args.get(i + 1).map(String::as_str));
    }
    // Type something into the bar, so what it finds can be looked at.
    if let Some(i) = args.iter().position(|a| a == "--type") {
        if let Some(t) = args.get(i + 1) {
            app.demo_type(t);
        }
    }
    // Choose several files, so a multiple selection can be looked at without
    // a pointer to make one with.
    // Put the pointer somewhere, so the hover lens can be looked at without a
    // pointer to hover with.
    if let Some(i) = args.iter().position(|a| a == "--point") {
        if let Some(at) = args.get(i + 1) {
            app.demo_point(at, w, h);
        }
    }
    if let Some(i) = args.iter().position(|a| a == "--choose") {
        if let Some(list) = args.get(i + 1) {
            app.demo_choose(list);
        }
    }
    if let Some(i) = args.iter().position(|a| a == "--demo-ask") {
        app.demo_ask(args.get(i + 1).map(String::as_str));
    }
    if let Some(i) = args.iter().position(|a| a == "--demo-history") {
        app.demo_history(args.get(i + 1).map(String::as_str));
    }

    if let Some(path) = shot {
        // Let any Expose land, so what is captured is a settled window.
        for _ in 0..8 {
            if let Event::Redraw | Event::Resized { .. } = window.wait(40) {
                window.draw(theme.backdrop_opaque, |c| app.render(c, &theme, w, h));
            }
        }
        match window.capture(&path) {
            Ok(()) => println!("captured {path}"),
            Err(e) => {
                eprintln!("nous: capture failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    loop {
        // A generous wait: this is a window, not a game. It wakes on input.
        // Anything reaching the bottom of this loop causes a frame, so an
        // event that changed nothing must `continue` instead.
        match window.wait(250) {
            Event::Close => break,
            Event::Redraw => {}
            Event::Resized { w: nw, h: nh } => {
                w = nw;
                h = nh;
            }
            Event::Key(k) => {
                if k.is(ffi::XK_Escape) && !app.handles_escape() {
                    break;
                }
                if k.ctrl && k.sym == 'q' as u64 {
                    break;
                }
                app.key(k, w, h);
            }
            Event::Text(t) => app.text(&t),
            // X reports the wheel as buttons four and five. Nothing else in
            // the interface treats them as clicks, so they are turned back into
            // scrolling here rather than in every view.
            Event::MouseDown {
                x,
                y,
                button,
                ctrl,
                shift,
            } => match button {
                4 => app.scroll(-3.0, w, h),
                5 => app.scroll(3.0, w, h),
                b => app.click(x, y, b, ctrl, shift, w, h),
            },
            // Releasing a button ends a scrub and changes nothing else.
            Event::MouseUp { x, y, button } => {
                app.release(x, y, button);
                continue;
            }
            // The pointer crossing the window is not news unless a menu is
            // open under it. X sends a great many motion events, and a frame
            // for each of them is what "glitchy when you drag the mouse over
            // it" looks like from the outside.
            Event::MouseMove { x, y } => {
                if !app.hover(x, y, w, h) {
                    continue;
                }
            }
            // A tick is the chance to pick up work put off between frames —
            // a preview that has since been made, a ledger gone stale. It
            // draws only if settling actually changed something.
            Event::Tick => {
                if !app.settle() {
                    continue;
                }
            }
            Event::FocusLost => continue,
        }
        app.settle();
        window.draw(theme.backdrop_opaque, |c| app.render(c, &theme, w, h));
    }
}

const HELP: &str = "\
nous — the interface window

    nous                     open it
    nous ~/Pictures          open on a folder
    nous --folder ~/Music    the same, said plainly
    nous --view files        open on a particular view
    nous --theme light       force a theme (dark, light)
    nous --check             say what is reachable and what is not
    nous --view player       (files, player, edit, history)
    nous --screenshot P.png  open, draw one frame, write it to P, exit

Inside the window:
    / or Ctrl-K  ask for something, about whatever you are looking at
    Enter        run the plan you were shown · Esc leaves it
    Ctrl-Z       take back the last thing that was done
    1 2 3 4      switch view · Tab for the next one
    arrows       move around · Return opens what is selected

In Files:
    F2 rename · Delete to trash · F5 refresh · Ctrl+C/X/V
    Ctrl+Shift+N new folder · Alt+Left/Right back and forward
    Backspace up a folder · type letters to jump to a file
    right-click for the rest

    Ctrl-Q       quit

Nothing is changed without the daemon: every rename, move and deletion
goes through it so that it is written down and can be taken back. Run
`nousd` if it is not already running. The command bar you summon over
other applications is separate, and has its own hotkey.
";

/// The offscreen probe every layout needs for measuring text before there is
/// anything to draw on.
pub fn probe() -> Image {
    Image::new(1, 1).expect("a one-pixel surface")
}
