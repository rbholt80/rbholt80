//! `nous-shell` — the graphical shell.
//!
//! A panel you summon with a key, type into, and dismiss. It is a real X11
//! window drawn with Cairo, not a browser pointed at a local server: nothing
//! has to stay running behind it and nothing is rendered by a web engine.
//!
//! The rule the whole interface is built on is that **nothing happens without
//! being shown first**. Every request is preflighted against the policy, and
//! anything that needs approval appears as a plan with each step's risk marked
//! before a single action runs.

mod context;
mod session;

use context::Context;
use nous_core::ipc::Client;
use nous_ui::panel::{Body, Layout, Panel};
use nous_ui::theme::{Metrics, Theme};
use nous_ui::window::{Event, Key, Window, WindowKind};
use session::{Job, Pending, Reply};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

/// Frame interval while something is animating. Sixty frames a second for a
/// pulsing marker is wasteful; the eye cannot tell at this size.
const ANIMATION_MS: i32 = 33;
/// How long to wait for an event when nothing is moving. Long enough that an
/// idle panel costs nothing, short enough that a reply arriving on the worker
/// thread is picked up without a perceptible delay.
const IDLE_MS: i32 = 80;

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let mut once = false;
    let mut initial = String::new();
    let mut capture = String::new();
    let mut context = Context::default();

    while let Some(a) = args.next() {
        match a.as_str() {
            "--once" => once = true,
            "--ask" => initial = args.next().unwrap_or_default(),
            "--capture" => capture = args.next().unwrap_or_default(),
            "--focus" => context.focus = args.next().filter(|s| !s.is_empty()),
            "--cwd" => context.cwd = args.next().filter(|s| !s.is_empty()),
            // Takes the rest of the arguments up to the next option, because a
            // file manager hands over a whole selection at once.
            "--paths" => {
                while let Some(p) = args.peek() {
                    if p.starts_with("--") {
                        break;
                    }
                    context.paths.push(args.next().unwrap_or_default());
                }
            }
            "--help" | "-h" => {
                println!("nous-shell [options]");
                println!();
                println!("  --ask TEXT      ask this straight away, without typing it");
                println!("  --once          close after one request rather than staying open");
                println!("  --paths FILE... attach a selection from the file manager");
                println!("  --cwd DIR       attach the folder being looked at");
                println!("  --focus TITLE   name the window that was in front");
                println!("  --capture PNG   save a picture of the panel, for reporting a fault");
                return;
            }
            other => {
                eprintln!("nous-shell: unknown option {other}");
                std::process::exit(2);
            }
        }
    }

    if let Err(e) = run(once, &initial, capture, context) {
        eprintln!("nous-shell: {e}");
        std::process::exit(1);
    }
}

fn run(once: bool, initial: &str, capture: String, context: Context) -> Result<(), String> {
    let theme = Theme::detect();
    let mut window = Window::open(
        "Nous",
        Metrics::PANEL_WIDTH as i32,
        Metrics::PROMPT_HEIGHT as i32,
        WindowKind::Overlay,
    )?;

    let (jobs, replies) = spawn_worker();
    let mut panel = Panel::new();
    panel.context = context.label();
    let mut pending: Option<Pending> = None;
    let mut busy = false;
    let mut height = 0.0f64;

    // Something asked for by name -- from the file manager's menu, or on the
    // command line -- is a request, not a draft. Pre-filling it and waiting for
    // Enter would make clicking "Tidy this folder" do nothing visible. Asking
    // is safe to do unprompted because it only produces a plan; nothing runs
    // until that plan is approved.
    if !initial.is_empty() {
        panel.input.set(initial);
        panel.set_body(Body::Working {
            note: format!("working out what \"{initial}\" means…"),
        });
        busy = true;
        let _ = jobs.send(Job::Ask(initial.to_string(), context.clone()));
    }

    // Draw before waiting for anything: the panel must be on screen the moment
    // it is summoned, not after the first event arrives.
    let mut dirty = true;

    loop {
        if dirty {
            let layout = relayout(&mut window, &panel, &theme, &mut height);
            let focused = !busy || matches!(panel.body, Body::Proposal { .. });
            window.draw(theme.backdrop_opaque, |c| {
                nous_ui::panel::render(c, &panel, &theme, &layout, focused);
            });
            dirty = false;
        }

        match replies.try_recv() {
            Ok(reply) => {
                busy = false;
                pending = reply.pending;
                panel.set_body(reply.body);
                dirty = true;
                if !capture.is_empty() {
                    settle(&mut window, &panel, &theme, &mut height);
                    window.capture(&capture)?;
                    if once {
                        return Ok(());
                    }
                }
                if once && !matches!(panel.body, Body::Proposal { .. }) {
                    // Give the result a moment to be readable before the
                    // window disappears out from under it.
                    settle(&mut window, &panel, &theme, &mut height);
                    return Ok(());
                }
                continue;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                if busy {
                    busy = false;
                    panel.set_body(Body::Error {
                        message: "the connection to nousd was lost".into(),
                    });
                    dirty = true;
                }
            }
        }

        let timeout = if busy { ANIMATION_MS } else { IDLE_MS };
        match window.wait(timeout) {
            Event::Close => return Ok(()),
            Event::Redraw | Event::Resized { .. } => dirty = true,
            Event::FocusLost => {
                // An overlay that stays visible after you click elsewhere is
                // the browser window the user did not want. It goes away.
                // Except while a plan is waiting: dismissing an unanswered
                // question because a notification stole focus would be worse.
                if !matches!(panel.body, Body::Proposal { .. }) {
                    return Ok(());
                }
                window.focus();
            }
            Event::Text(t) => {
                panel.input.insert(&t);
                dirty = true;
            }
            Event::Key(k) => match act(&mut panel, &mut pending, &mut busy, &jobs, k, &context) {
                Action::Redraw => dirty = true,
                Action::Quit => return Ok(()),
                Action::Nothing => {}
            },
            Event::MouseDown { y, .. } => {
                // Clicking outside the panel dismisses it, the same as losing
                // focus. Inside, a click is just a way to take focus back.
                if y < 0.0 || y > height {
                    return Ok(());
                }
            }
            Event::MouseUp { .. } | Event::MouseMove { .. } => {}
            Event::Tick => {
                if busy {
                    panel.phase += ANIMATION_MS as f64 / 1000.0;
                    dirty = true;
                }
            }
        }
    }
}

enum Action {
    Redraw,
    Quit,
    Nothing,
}

// X11 keysym names are lower-cased constants; renaming them here would make
// them impossible to check against keysymdef.h.
#[allow(non_upper_case_globals)]
fn act(
    panel: &mut Panel,
    pending: &mut Option<Pending>,
    busy: &mut bool,
    jobs: &Sender<Job>,
    k: Key,
    context: &Context,
) -> Action {
    use nous_ui::ffi::*;
    use nous_ui::input::Step;

    let step = if k.ctrl { Step::Word } else { Step::Char };

    match k.sym {
        XK_Escape => {
            // Escape backs out one layer at a time rather than closing
            // outright: discarding a plan and closing the panel are different
            // intentions and should not share a key press.
            if let Some(p) = pending.take() {
                let _ = p;
                panel.set_body(Body::Empty);
                return Action::Redraw;
            }
            if panel.body != Body::Empty {
                panel.set_body(Body::Empty);
                return Action::Redraw;
            }
            if !panel.input.is_empty() {
                panel.input.clear();
                return Action::Redraw;
            }
            Action::Quit
        }
        XK_Return | XK_KP_Enter => {
            if *busy {
                return Action::Nothing;
            }
            if let Some(p) = pending.take() {
                *busy = true;
                panel.set_body(Body::Working {
                    note: "applying…".into(),
                });
                let _ = jobs.send(Job::Approve(p));
                return Action::Redraw;
            }
            let text = panel.input.text().trim().to_string();
            if text.is_empty() {
                return Action::Nothing;
            }
            *busy = true;
            panel.set_body(Body::Working {
                note: format!("working out what \"{text}\" means…"),
            });
            let _ = jobs.send(Job::Ask(text, context.clone()));
            Action::Redraw
        }
        XK_BackSpace => {
            panel.input.backspace(step);
            Action::Redraw
        }
        XK_Delete => {
            panel.input.delete(step);
            Action::Redraw
        }
        XK_Left => {
            panel.input.move_caret(-1, step, k.shift);
            Action::Redraw
        }
        XK_Right => {
            panel.input.move_caret(1, step, k.shift);
            Action::Redraw
        }
        XK_Home => {
            panel.input.move_caret(-1, Step::Line, k.shift);
            Action::Redraw
        }
        XK_End => {
            panel.input.move_caret(1, Step::Line, k.shift);
            Action::Redraw
        }
        XK_Up | XK_Down => {
            let delta = if k.sym == XK_Up { -1 } else { 1 };
            let visible = visible_rows(panel);
            panel.move_selection(delta, visible);
            Action::Redraw
        }
        XK_Page_Up | XK_Page_Down => {
            let visible = visible_rows(panel).max(1) as i32;
            let delta = if k.sym == XK_Page_Up {
                -visible
            } else {
                visible
            };
            panel.move_selection(delta, visible as usize);
            Action::Redraw
        }
        // Ctrl+A selects, Ctrl+Z undoes the last thing the system did.
        0x61 | 0x41 if k.ctrl => {
            panel.input.select_all();
            Action::Redraw
        }
        0x7a | 0x5a if k.ctrl => {
            if *busy {
                return Action::Nothing;
            }
            if matches!(
                panel.body,
                Body::Done {
                    undo_hint: true,
                    ..
                }
            ) {
                *busy = true;
                panel.set_body(Body::Working {
                    note: "undoing…".into(),
                });
                let _ = jobs.send(Job::Undo);
                return Action::Redraw;
            }
            Action::Nothing
        }
        0x75 | 0x55 if k.ctrl => {
            panel.input.clear();
            Action::Redraw
        }
        _ => Action::Nothing,
    }
}

/// How many steps are on screen, so paging moves by what the eye can see.
fn visible_rows(panel: &Panel) -> usize {
    // Derived from the same metrics the layout uses, rather than re-running a
    // layout that would need a live canvas.
    let list = Metrics::PANEL_MAX_HEIGHT - Metrics::PROMPT_HEIGHT - 30.0 - Metrics::PAD - 44.0;
    let n = (list / Metrics::ROW_HEIGHT).floor().max(1.0) as usize;
    n.min(panel.steps().len().max(1))
}

/// Recompute the layout and resize the window to match, so the panel is always
/// exactly as tall as what it is showing.
fn relayout(window: &mut Window, panel: &Panel, theme: &Theme, height: &mut f64) -> Layout {
    let (w, _) = window.size();
    // A throwaway surface, purely to measure text with. Measuring against the
    // window's own surface would need a frame in progress.
    let probe = nous_ui::draw::Image::new(1, 1).expect("a 1x1 surface");
    let layout = Layout::compute(panel, w, &probe.canvas(), theme);
    if (layout.height - *height).abs() > 0.5 {
        window.resize(w as i32, layout.height.ceil() as i32);
        *height = layout.height;
    }
    layout
}

/// Draw a final frame and hold it briefly so a one-shot result can be read.
fn settle(window: &mut Window, panel: &Panel, theme: &Theme, height: &mut f64) {
    let layout = relayout(window, panel, theme, height);
    window.draw(theme.backdrop_opaque, |c| {
        nous_ui::panel::render(c, panel, theme, &layout, false);
    });
    window.sync();
    std::thread::sleep(std::time::Duration::from_millis(900));
}

/// Start the thread that talks to the daemon.
///
/// It owns the connection for its whole life. Reconnecting per request would
/// add a round trip to every keystroke-driven action, and a socket that has
/// gone away is reported as a lost connection rather than retried silently.
fn spawn_worker() -> (Sender<Job>, Receiver<Reply>) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<Job>();
    let (reply_tx, reply_rx) = std::sync::mpsc::channel::<Reply>();

    std::thread::spawn(move || {
        let mut client = match Client::connect() {
            Ok(c) => c,
            Err(e) => {
                // Drain jobs and answer each with the same explanation, rather
                // than leaving the panel spinning forever.
                while job_rx.recv().is_ok() {
                    let sent = reply_tx.send(Reply {
                        body: Body::Error {
                            message: format!(
                                "nousd is not running ({e}).\n\
                                 start it with: systemctl --user start nousd"
                            ),
                        },
                        pending: None,
                    });
                    if sent.is_err() {
                        return;
                    }
                }
                return;
            }
        };
        while let Ok(job) = job_rx.recv() {
            let reply = session::run(&mut client, job);
            if reply_tx.send(reply).is_err() {
                return;
            }
        }
    });

    (job_tx, reply_rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::json::Json;
    use nous_ui::ffi::*;
    use nous_ui::panel::Step as PanelStep;
    use nous_ui::theme::Risk;

    struct Harness {
        panel: Panel,
        pending: Option<Pending>,
        busy: bool,
        jobs: Sender<Job>,
        sent: Receiver<Job>,
    }

    impl Harness {
        fn new() -> Harness {
            let (jobs, sent) = std::sync::mpsc::channel();
            Harness {
                panel: Panel::new(),
                pending: None,
                busy: false,
                jobs,
                sent,
            }
        }

        fn press(&mut self, sym: u64) -> Action {
            self.press_with(Key {
                sym,
                ctrl: false,
                shift: false,
                alt: false,
            })
        }

        fn press_with(&mut self, k: Key) -> Action {
            act(
                &mut self.panel,
                &mut self.pending,
                &mut self.busy,
                &self.jobs,
                k,
                &Context::default(),
            )
        }

        fn job(&self) -> Option<Job> {
            self.sent.try_recv().ok()
        }
    }

    fn a_proposal() -> (Body, Pending) {
        (
            Body::Proposal {
                headline: "2 changes".into(),
                steps: vec![PanelStep {
                    capability: "fs.move:~/Downloads/a".into(),
                    summary: "move a".into(),
                    risk: Risk::Write,
                }],
            },
            Pending::Curate(Json::obj()),
        )
    }

    #[test]
    fn enter_on_a_proposal_approves_it() {
        // The bug this exists for: a plan was shown with no way to say yes.
        let mut h = Harness::new();
        let (body, pending) = a_proposal();
        h.panel.set_body(body);
        h.pending = Some(pending.clone());

        h.press(XK_Return);
        match h.job() {
            Some(Job::Approve(p)) => assert_eq!(p, pending),
            other => panic!("enter did not approve the plan: {other:?}"),
        }
        assert!(h.busy, "the panel must show it is working");
        assert!(
            h.pending.is_none(),
            "the plan must not be approvable twice by holding enter"
        );
    }

    #[test]
    fn escape_on_a_proposal_discards_it_without_approving() {
        let mut h = Harness::new();
        let (body, pending) = a_proposal();
        h.panel.set_body(body);
        h.pending = Some(pending);

        h.press(XK_Escape);
        assert!(h.job().is_none(), "escape sent something to the daemon");
        assert!(h.pending.is_none());
        assert_eq!(h.panel.body, Body::Empty);
        assert!(!h.busy);
    }

    #[test]
    fn escape_backs_out_one_layer_at_a_time() {
        let mut h = Harness::new();
        h.panel.input.set("tidy my downloads");
        h.panel.set_body(Body::Done {
            headline: "done".into(),
            detail: String::new(),
            undo_hint: false,
        });

        assert!(matches!(h.press(XK_Escape), Action::Redraw));
        assert_eq!(h.panel.body, Body::Empty, "the result is cleared first");
        assert_eq!(
            h.panel.input.text(),
            "tidy my downloads",
            "but not the text"
        );

        assert!(matches!(h.press(XK_Escape), Action::Redraw));
        assert!(h.panel.input.is_empty(), "then the text");

        assert!(
            matches!(h.press(XK_Escape), Action::Quit),
            "and only then does the panel close"
        );
    }

    #[test]
    fn enter_on_an_empty_prompt_asks_nothing() {
        let mut h = Harness::new();
        h.press(XK_Return);
        assert!(h.job().is_none());
        assert!(!h.busy);

        // Nor does whitespace count as a question.
        h.panel.input.set("   ");
        h.press(XK_Return);
        assert!(h.job().is_none(), "blank input was sent as a request");
    }

    #[test]
    fn enter_submits_what_was_typed_with_the_surrounding_whitespace_removed() {
        let mut h = Harness::new();
        h.panel.input.set("  tidy my downloads  ");
        h.press(XK_Return);
        match h.job() {
            Some(Job::Ask(text, _)) => assert_eq!(text, "tidy my downloads"),
            other => panic!("expected a question, got {other:?}"),
        }
        assert!(h.busy);
    }

    #[test]
    fn a_second_enter_while_working_does_not_send_the_request_twice() {
        let mut h = Harness::new();
        h.panel.input.set("tidy my downloads");
        h.press(XK_Return);
        assert!(h.job().is_some());

        h.press(XK_Return);
        assert!(
            h.job().is_none(),
            "holding enter queued a second identical request"
        );
    }

    #[test]
    fn undo_is_only_sent_when_there_is_something_to_undo() {
        let ctrl_z = Key {
            sym: 0x7a,
            ctrl: true,
            shift: false,
            alt: false,
        };

        let mut h = Harness::new();
        h.panel.set_body(Body::Done {
            headline: "moved 3 files".into(),
            detail: String::new(),
            undo_hint: false,
        });
        h.press_with(ctrl_z);
        assert!(h.job().is_none(), "undid something that cannot be undone");

        h.panel.set_body(Body::Done {
            headline: "moved 3 files".into(),
            detail: String::new(),
            undo_hint: true,
        });
        h.press_with(ctrl_z);
        assert!(matches!(h.job(), Some(Job::Undo)));
        assert!(h.busy);
    }

    #[test]
    fn ctrl_z_without_ctrl_types_a_z() {
        let mut h = Harness::new();
        h.panel.set_body(Body::Done {
            headline: "moved 3 files".into(),
            detail: String::new(),
            undo_hint: true,
        });
        h.press(0x7a);
        assert!(h.job().is_none(), "a bare z undid the last action");
    }

    #[test]
    fn arrows_move_the_caret_but_ctrl_arrows_move_by_word() {
        let mut h = Harness::new();
        h.panel.input.set("tidy my downloads");
        h.press(XK_Home);
        assert_eq!(h.panel.input.caret(), 0);

        h.press_with(Key {
            sym: XK_Right,
            ctrl: true,
            shift: false,
            alt: false,
        });
        assert_eq!(h.panel.input.caret(), 5, "ctrl-right skipped a whole word");

        h.press(XK_Right);
        assert_eq!(
            h.panel.input.caret(),
            6,
            "a plain right moved one character"
        );
    }

    #[test]
    fn arrows_review_the_plan_when_one_is_showing() {
        let mut h = Harness::new();
        h.panel.set_body(Body::Proposal {
            headline: "many".into(),
            steps: (0..5)
                .map(|i| PanelStep {
                    capability: format!("fs.move:~/x{i}"),
                    summary: format!("move x{i}"),
                    risk: Risk::Write,
                })
                .collect(),
        });
        h.press(XK_Down);
        h.press(XK_Down);
        assert_eq!(h.panel.selected, 2);
        h.press(XK_Up);
        assert_eq!(h.panel.selected, 1);
        assert!(h.job().is_none(), "reviewing a plan must not run any of it");
    }
}
