//! Playing something, and cutting it.
//!
//! One surface for both, because they are the same act at different speeds:
//! you watch a thing, you find the moment it should start, you say so. Putting
//! the timeline under the picture rather than behind a separate "edit" mode
//! means the cut is made where the material is being looked at.
//!
//! Every number here comes from the daemon — durations from `media.probe`,
//! position from the running player, clips from the edit project on disk. The
//! view computes no timing of its own, so what is drawn is what is true.

use crate::draw::{Canvas, Picture, Rect, Rgba};
use crate::theme::{Metrics, Theme};
use nous_core::json::{json_obj, Json};
use std::collections::HashMap;

/// One piece of the timeline: a whole file, or the part of it that survives.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub id: String,
    pub path: String,
    pub name: String,
    /// Where this clip starts inside its source file, in seconds.
    pub start: f64,
    /// Where it ends. Never past the source's own duration.
    pub end: f64,
    /// The whole length of the source, so the trimmed-away part can be drawn
    /// rather than merely implied.
    pub source_duration: f64,
    pub speed: f64,
    pub volume: f64,
    /// A cached PNG frame, for the timeline and the stage.
    pub thumb: Option<String>,
}

impl Clip {
    /// How long this clip lasts once its trim and speed are applied. The
    /// number that matters for the finished piece, and the one the timeline is
    /// measured in.
    pub fn duration(&self) -> f64 {
        let span = (self.end - self.start).max(0.0);
        if self.speed > 0.0 {
            span / self.speed
        } else {
            span
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Stopped,
    Playing,
    Paused,
}

/// What the player is showing and doing.
pub struct Player {
    /// The project's name, which is what `media.render` is told to render.
    pub project: String,
    pub clips: Vec<Clip>,
    pub selected: usize,
    pub transport: Transport,
    /// Position within the *timeline*, not within any one file.
    pub position: f64,
    pub volume: f64,
    /// Set while a render is running, so the surface can say so instead of
    /// looking idle for two minutes.
    pub rendering: Option<String>,
    cache: HashMap<String, Option<Picture>>,
}

impl Player {
    pub fn new(project: &str, clips: Vec<Clip>) -> Player {
        Player {
            project: project.to_string(),
            clips,
            selected: 0,
            transport: Transport::Stopped,
            position: 0.0,
            volume: 1.0,
            rendering: None,
            cache: HashMap::new(),
        }
    }

    /// Build the view from the edit project the daemon keeps on disk.
    ///
    /// The document is the truth; this reads it, it never writes it. `thumb`
    /// is asked for a cached frame per source path, because where those live
    /// is the daemon's business, not the view's.
    pub fn from_project(project: &Json, mut thumb: impl FnMut(&str) -> Option<String>) -> Player {
        let name = project.str_or("name", "untitled").to_string();
        let clips = project
            .arr_or_empty("clips")
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let path = c.str_or("path", "").to_string();
                let end = c.f64_or("out", 0.0);
                Clip {
                    id: {
                        let id = c.str_or("id", "");
                        if id.is_empty() {
                            format!("c{}", i + 1)
                        } else {
                            id.to_string()
                        }
                    },
                    name: path
                        .rsplit('/')
                        .next()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&path)
                        .to_string(),
                    start: c.f64_or("in", 0.0),
                    end,
                    // A document written before clips carried their source's
                    // length has only its out-point to go on. Treating that as
                    // the whole file draws the clip as untrimmed, which is
                    // what an untrimmed clip in such a document is.
                    source_duration: c.f64_or("duration", end).max(end),
                    speed: {
                        let v = c.f64_or("speed", 1.0);
                        if v > 0.0 {
                            v
                        } else {
                            1.0
                        }
                    },
                    volume: c.f64_or("volume", 1.0),
                    thumb: thumb(&path),
                    path,
                }
            })
            .collect();
        Player::new(&name, clips)
    }

    pub fn selected_clip(&self) -> Option<&Clip> {
        self.clips.get(self.selected)
    }

    /// The finished length of everything on the timeline.
    pub fn duration(&self) -> f64 {
        self.clips.iter().map(Clip::duration).sum()
    }

    /// Where on the timeline clip `i` begins.
    pub fn clip_start(&self, i: usize) -> f64 {
        self.clips.iter().take(i).map(Clip::duration).sum()
    }

    /// Which clip is playing at `t`, and how far into it. `None` past the end.
    pub fn at(&self, t: f64) -> Option<(usize, f64)> {
        let mut acc = 0.0;
        for (i, c) in self.clips.iter().enumerate() {
            let d = c.duration();
            if t < acc + d || (i == self.clips.len() - 1 && (t - (acc + d)).abs() < 1e-9) {
                return Some((i, (t - acc).max(0.0)));
            }
            acc += d;
        }
        None
    }

    pub fn seek(&mut self, to: f64) {
        self.position = to.clamp(0.0, self.duration());
    }

    /// Move by `dt` seconds, stopping at both ends rather than wrapping.
    pub fn nudge(&mut self, dt: f64) {
        self.seek(self.position + dt);
    }

    pub fn select(&mut self, i: usize) {
        if i < self.clips.len() {
            self.selected = i;
        }
    }

    /// Put the playhead at the start of the selected clip. What "go to this
    /// clip" means when you click one.
    pub fn seek_to_selected(&mut self) {
        let at = self.clip_start(self.selected);
        self.seek(at);
    }

    /// Set the in-point of the selected clip to wherever the playhead is.
    ///
    /// Returns the new in-point in *source* seconds, which is what
    /// `media.edit op=trim` wants — the daemon holds the truth, this only says
    /// where. Refuses to make a clip that ends before it starts.
    pub fn mark_in(&mut self) -> Option<(String, f64)> {
        let i = self.cut_target()?;
        self.selected = i;
        let base = self.clip_start(i);
        let c = self.clips.get_mut(i)?;
        let into_clip = (self.position - base).max(0.0) * c.speed;
        let new_start = (c.start + into_clip).min(c.end - MIN_CLIP);
        if new_start <= c.start + 1e-9 && into_clip > 1e-9 {
            return None;
        }
        c.start = new_start.max(0.0);
        Some((c.id.clone(), c.start))
    }

    /// Set the out-point of the selected clip to the playhead.
    pub fn mark_out(&mut self) -> Option<(String, f64)> {
        let i = self.cut_target()?;
        self.selected = i;
        let base = self.clip_start(i);
        let c = self.clips.get_mut(i)?;
        let into_clip = (self.position - base).max(0.0) * c.speed;
        let new_end = (c.start + into_clip).clamp(c.start + MIN_CLIP, c.source_duration);
        c.end = new_end;
        Some((c.id.clone(), c.end))
    }

    /// Which clip a cut applies to: the one under the playhead.
    ///
    /// Not the highlighted one. Marking is done while watching, so by the time
    /// the moment arrives the playhead has usually left whatever was clicked —
    /// and cutting the highlighted clip then means trimming the wrong file by
    /// an offset measured from the wrong place. Past the end of the material
    /// there is nothing under the playhead, so the highlight stands in.
    fn cut_target(&self) -> Option<usize> {
        match self.at(self.position) {
            Some((i, _)) => Some(i),
            None if self.selected < self.clips.len() => Some(self.selected),
            None => None,
        }
    }

    fn picture(&mut self, path: &str) -> Option<&Picture> {
        self.cache
            .entry(path.to_string())
            .or_insert_with(|| Picture::load(path).ok())
            .as_ref()
    }
}

/// What the surface asks the daemon to do.
///
/// The view never edits a file, never moves a playhead in mpv, never runs
/// ffmpeg. It says what was meant, in the same capability vocabulary everything
/// else in the system speaks, and hands it over to be adjudicated, executed and
/// journalled like any other request. That is what makes a cut undoable: it
/// went through the broker rather than around it.
#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub cap: &'static str,
    pub args: Json,
}

/// A gesture on the surface, before it has been given a meaning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    PlayPause,
    /// Jump to the start of the previous or next clip.
    Prev,
    Next,
    /// Move by a number of seconds, positive or negative.
    Nudge(f64),
    /// Go to an absolute position on the timeline.
    SeekTo(f64),
    /// Make the playhead the selected clip's new start, or new end.
    MarkIn,
    MarkOut,
    Select(usize),
    /// 0.0 to 1.0.
    SetVolume(f64),
    Render,
}

impl Player {
    /// Apply a gesture, and say what the daemon must be asked for.
    ///
    /// Local state moves immediately so the surface does not sit still waiting
    /// for a round trip; the returned intent is what makes it true. `None`
    /// means the gesture was entirely the view's own business — which clip is
    /// highlighted is not something the system needs to be told.
    pub fn apply(&mut self, cmd: Command) -> Option<Intent> {
        match cmd {
            Command::PlayPause => {
                self.transport = match self.transport {
                    Transport::Playing => Transport::Paused,
                    _ => Transport::Playing,
                };
                Some(Intent {
                    cap: "media.control",
                    args: json_obj([("action", "toggle".into())]),
                })
            }
            Command::Prev => {
                // The first press goes to the head of the clip you are in;
                // pressing again from there goes back one. Which is what
                // "previous" does everywhere, and it saves a second gesture
                // for the far more common "start this bit again".
                let (i, into) = self.at(self.position)?;
                let target = if into > 1.0 || i == 0 { i } else { i - 1 };
                self.select(target);
                self.seek_to_selected();
                Some(self.seek_intent())
            }
            Command::Next => {
                let (i, _) = self.at(self.position)?;
                if i + 1 < self.clips.len() {
                    self.select(i + 1);
                    self.seek_to_selected();
                } else {
                    self.seek(self.duration());
                }
                Some(self.seek_intent())
            }
            Command::Nudge(dt) => {
                self.nudge(dt);
                Some(self.seek_intent())
            }
            Command::SeekTo(t) => {
                self.seek(t);
                Some(self.seek_intent())
            }
            Command::MarkIn => {
                let (clip, at) = self.mark_in()?;
                Some(Intent {
                    cap: "media.edit",
                    args: json_obj([
                        ("project", self.project.clone().into()),
                        ("op", "trim".into()),
                        ("clip", clip.into()),
                        ("in", at.into()),
                    ]),
                })
            }
            Command::MarkOut => {
                let (clip, at) = self.mark_out()?;
                let start = self.clips.iter().find(|c| c.id == clip)?.start;
                Some(Intent {
                    cap: "media.edit",
                    args: json_obj([
                        ("project", self.project.clone().into()),
                        ("op", "trim".into()),
                        ("clip", clip.into()),
                        // Both ends every time: `trim` writes what it is
                        // given, and sending only the out-point would leave
                        // the daemon's idea of the in-point to chance.
                        ("in", start.into()),
                        ("out", at.into()),
                    ]),
                })
            }
            Command::Select(i) => {
                self.select(i);
                None
            }
            Command::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                Some(Intent {
                    cap: "media.control",
                    args: json_obj([
                        ("action", "volume".into()),
                        // mpv counts volume out of 100, the surface out of one.
                        ("level", (self.volume * 100.0).into()),
                    ]),
                })
            }
            Command::Render => {
                self.rendering = Some(format!("rendering {}…", self.project));
                Some(Intent {
                    cap: "media.render",
                    args: json_obj([("project", self.project.clone().into())]),
                })
            }
        }
    }

    fn seek_intent(&self) -> Intent {
        Intent {
            cap: "media.control",
            args: json_obj([("action", "seek".into()), ("to", self.position.into())]),
        }
    }
}

/// A clip shorter than this is a mistake, not an edit.
const MIN_CLIP: f64 = 0.1;

/// `1:04` or `1:02:03`. Hours only when there are hours: a three-minute song
/// does not need to be told it is not three hours long.
pub fn timecode(seconds: f64) -> String {
    let t = seconds.max(0.0).round() as u64;
    let (h, m, s) = (t / 3600, (t % 3600) / 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

// --- layout ---------------------------------------------------------------

const TRANSPORT_H: f64 = 56.0;
const TIMELINE_H: f64 = 78.0;
const SCRUB_H: f64 = 26.0;
/// The strip under each clip showing the whole source file, with the kept part
/// lit. Tall enough to read at a glance, and clear of the selection ring so the
/// two never blend into one another.
const RIBBON_H: f64 = 4.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub panel: Rect,
    /// Where the picture goes.
    pub stage: Rect,
    /// The single bar showing position in the whole timeline.
    pub scrub: Rect,
    pub transport: Rect,
    /// The clips, laid out in proportion to their finished length.
    pub timeline: Rect,
    pub rows: Vec<Rect>,
}

impl Layout {
    pub fn compute(player: &Player, width: f64, height: f64) -> Layout {
        let pad = Metrics::PAD;
        let inner = (width - pad * 2.0).max(0.0);
        // The timeline only appears when there is something on it. A player
        // with one song does not need an editing surface.
        let timeline_h = if player.clips.len() > 1 {
            TIMELINE_H
        } else {
            0.0
        };

        let bottom = SCRUB_H + TRANSPORT_H + timeline_h + pad;
        let stage_h = (height - bottom).max(0.0);

        let scrub_y = stage_h;
        let transport_y = scrub_y + SCRUB_H;
        let timeline_y = transport_y + TRANSPORT_H;

        let mut rows = Vec::new();
        if timeline_h > 0.0 {
            let total = player.duration();
            let gap = 4.0;
            let usable = (inner - gap * (player.clips.len().saturating_sub(1)) as f64).max(1.0);
            let mut x = pad;
            for c in &player.clips {
                // Proportional to finished length, with a floor so a
                // half-second clip is still big enough to aim at.
                let share = if total > 0.0 {
                    c.duration() / total
                } else {
                    0.0
                };
                let w = (usable * share).max(18.0);
                rows.push(Rect::new(x, timeline_y + 20.0, w, timeline_h - 28.0));
                x += w + gap;
            }
        }

        Layout {
            panel: Rect::new(0.0, 0.0, width, height),
            stage: Rect::new(0.0, 0.0, width, stage_h),
            scrub: Rect::new(pad, scrub_y, inner, SCRUB_H),
            transport: Rect::new(pad, transport_y, inner, TRANSPORT_H),
            timeline: Rect::new(pad, timeline_y, inner, timeline_h),
            rows,
        }
    }

    /// The time a point on the scrub bar corresponds to, as a fraction.
    pub fn scrub_fraction(&self, x: f64) -> f64 {
        if self.scrub.w <= 0.0 {
            return 0.0;
        }
        ((x - self.scrub.x) / self.scrub.w).clamp(0.0, 1.0)
    }

    pub fn clip_at(&self, x: f64, y: f64) -> Option<usize> {
        self.rows.iter().position(|r| r.contains(x, y))
    }

    /// Where the playhead falls on the timeline, in pixels.
    ///
    /// Taken from the clip actually playing and how far into it we are, not
    /// from a fraction of the timeline's total width. The two are not the
    /// same: rows have a minimum width and gaps between them, so a short clip
    /// takes more width than its share of the running time. Interpolating
    /// across the whole strip would drift away from the clip boundaries
    /// exactly when the timeline holds a very short clip.
    pub fn playhead_x(&self, player: &Player) -> Option<f64> {
        if self.rows.is_empty() {
            return None;
        }
        match player.at(player.position) {
            Some((i, into)) => {
                let row = self.rows.get(i)?;
                let d = player.clips.get(i)?.duration();
                let f = if d > 0.0 {
                    (into / d).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                Some(row.x + row.w * f)
            }
            // Past the last clip: park it at the end of the material.
            None => self.rows.last().map(|r| r.right()),
        }
    }
}

// --- drawing --------------------------------------------------------------

pub fn render(c: &Canvas, player: &mut Player, theme: &Theme, layout: &Layout) {
    c.fill_rect(layout.panel, theme.backdrop_opaque);
    draw_stage(c, player, theme, layout);
    draw_scrub(c, player, theme, layout);
    draw_transport(c, player, theme, layout);
    if layout.timeline.h > 0.0 {
        draw_timeline(c, player, theme, layout);
    }
}

fn draw_stage(c: &Canvas, player: &mut Player, theme: &Theme, layout: &Layout) {
    let s = layout.stage;
    if s.h <= 0.0 {
        return;
    }
    // Black behind the picture whatever the theme. A photograph or a film
    // frame is judged against black; a pale surround changes what it looks
    // like, and this is the one place the theme should not have an opinion.
    c.fill_rect(s, Rgba::rgb(0, 0, 0));

    // The frame of whatever is playing now, not of whatever is selected: the
    // stage follows the playhead.
    let showing = player
        .at(player.position)
        .map(|(i, _)| i)
        .unwrap_or(player.selected);
    let thumb = player.clips.get(showing).and_then(|c| c.thumb.clone());

    let mut drew = false;
    if let Some(path) = thumb {
        if let Some(pic) = player.picture(&path) {
            let into = pic.contain(s.inset(Metrics::PAD));
            c.picture(pic, into);
            drew = true;
        }
    }
    if !drew {
        let name = player
            .clips
            .get(showing)
            .map_or("nothing to play", |c| c.name.as_str());
        let (w, h) = c.measure(name, &theme.title_font(), Some(s.w * 0.8));
        c.text(
            name,
            s.x + (s.w - w) / 2.0,
            s.y + (s.h - h) / 2.0,
            &theme.title_font(),
            theme.text_dim,
            Some(s.w * 0.8),
        );
    }
}

/// One bar for the whole piece: where you are, and how much is left.
fn draw_scrub(c: &Canvas, player: &Player, theme: &Theme, layout: &Layout) {
    let s = layout.scrub;
    let track = Rect::new(s.x, s.y + s.h / 2.0 - 2.0, s.w, 4.0);
    c.fill_rounded(track, 2.0, theme.surface_active);

    let total = player.duration();
    let at = if total > 0.0 {
        (player.position / total).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if at > 0.0 {
        c.fill_rounded(
            Rect::new(track.x, track.y, track.w * at, track.h),
            2.0,
            theme.voice,
        );
    }
    // The playhead, big enough to grab.
    c.fill_circle(
        track.x + track.w * at,
        track.y + track.h / 2.0,
        6.0,
        theme.voice,
    );
}

fn draw_transport(c: &Canvas, player: &Player, theme: &Theme, layout: &Layout) {
    let t = layout.transport;
    let small = theme.small_font();
    let cy = t.y + t.h / 2.0;

    // Left: where you are, out of how long. Tabular so the digits do not
    // jitter as the seconds tick over.
    let clock = format!(
        "{} / {}",
        timecode(player.position),
        timecode(player.duration())
    );
    let (_, ch) = c.measure(&clock, &theme.font_mono, None);
    c.text(
        &clock,
        t.x,
        cy - ch / 2.0,
        &theme.font_mono,
        theme.text,
        None,
    );

    // Middle: the three controls, drawn rather than lettered.
    let mid = t.x + t.w / 2.0;
    let colour = theme.text;
    glyph_prev(c, mid - 44.0, cy, colour);
    match player.transport {
        Transport::Playing => glyph_pause(c, mid, cy, theme.voice),
        _ => glyph_play(c, mid, cy, theme.voice),
    }
    glyph_next(c, mid + 44.0, cy, colour);

    // Right: what is happening, or what the keys do.
    let note = match player.rendering.as_deref() {
        Some(what) => what.to_string(),
        None if player.clips.len() > 1 => "i  in    o  out    r  render".to_string(),
        None => "space  play/pause".to_string(),
    };
    let (nw, nh) = c.measure(&note, &small, None);
    c.text(
        &note,
        t.right() - nw,
        cy - nh / 2.0,
        &small,
        if player.rendering.is_some() {
            theme.voice
        } else {
            theme.text_faint
        },
        None,
    );
}

/// The clips, side by side, each as wide as its share of the finished piece.
///
/// The trimmed-away part of a clip is drawn faintly rather than removed, so a
/// cut reads as a decision that can be revisited instead of material that has
/// vanished.
fn draw_timeline(c: &Canvas, player: &mut Player, theme: &Theme, layout: &Layout) {
    let tl = layout.timeline;
    let small = theme.small_font();
    c.text("TIMELINE", tl.x, tl.y + 2.0, &small, theme.text_faint, None);

    let selected = player.selected;

    for (i, row) in layout.rows.iter().enumerate() {
        let Some(clip) = player.clips.get(i).cloned() else {
            continue;
        };
        let radius = Metrics::RADIUS_SMALL / 2.0;
        c.fill_rounded(*row, radius, theme.surface);

        // Everything from here is clipped to the row, so a long title cannot
        // spill into the clip beside it and a frame cannot square off the
        // rounded corners.
        c.clip_rect(*row);

        // The frame as the row's ground. A timeline is read by eye before it
        // is read by name, and a strip of flat rectangles gives the eye
        // nothing to work with. Darkened, because the caption has to stay
        // legible over whatever the picture happens to be.
        if let Some(path) = clip.thumb.clone() {
            if let Some(pic) = player.picture(&path) {
                let into = pic.cover(*row);
                c.picture(pic, into);
            }
            c.fill_rect(*row, theme.backdrop_opaque.with_alpha(0.62));
        }

        // What the cut kept, against the whole source file. The row itself is
        // as wide as the clip's *finished* length, so the trimmed-away part
        // cannot be drawn inside it without claiming the row spans the whole
        // source. This ribbon does span the whole source: the lit part is what
        // survives, and its position says where in the file the cut was made.
        if clip.source_duration > 0.0 {
            let ribbon = Rect::new(row.x, row.bottom() - RIBBON_H, row.w, RIBBON_H);
            c.fill_rect(ribbon, theme.hairline);
            let a = (clip.start / clip.source_duration).clamp(0.0, 1.0);
            let b = (clip.end / clip.source_duration).clamp(a, 1.0);
            c.fill_rect(
                Rect::new(
                    ribbon.x + ribbon.w * a,
                    ribbon.y,
                    ribbon.w * (b - a),
                    RIBBON_H,
                ),
                if i == selected {
                    theme.voice
                } else {
                    theme.text_dim
                },
            );
        }
        c.text(
            &clip.name,
            row.x + 6.0,
            row.y + 5.0,
            &small,
            theme.text,
            Some((row.w - 12.0).max(0.0)),
        );
        c.text(
            &timecode(clip.duration()),
            row.x + 6.0,
            row.bottom() - 21.0,
            &small,
            theme.text_dim,
            Some((row.w - 12.0).max(0.0)),
        );
        c.restore();

        // Last, and outside the clip, so the ring is a whole outline rather
        // than one the row's own edge has eaten.
        if i == selected {
            c.stroke_rounded(row.inset(0.75), radius, 1.5, theme.voice);
        }
    }

    // The playhead, drawn across the timeline in the same colour it has on the
    // scrub bar, so the two readings are obviously one position.
    if let Some(x) = layout.playhead_x(player) {
        let first = layout.rows[0];
        c.line(x, first.y - 4.0, x, first.bottom() + 4.0, 1.5, theme.voice);
    }
}

// --- transport glyphs ------------------------------------------------------
// Drawn rather than set in a font: the shapes are three triangles and two
// bars, and a font that happens to lack them would leave empty boxes where the
// controls should be.

fn glyph_play(c: &Canvas, cx: f64, cy: f64, colour: Rgba) {
    let r = 9.0;
    // A filled triangle, built from horizontal lines: no polygon primitive is
    // needed for a shape this simple.
    let steps = 18;
    for i in 0..steps {
        let t = i as f64 / steps as f64;
        let half = r * (1.0 - t);
        let x = cx - r * 0.6 + t * r * 1.4;
        c.line(
            x,
            cy - half,
            x,
            cy + half,
            r * 1.4 / steps as f64 + 0.6,
            colour,
        );
    }
}

fn glyph_pause(c: &Canvas, cx: f64, cy: f64, colour: Rgba) {
    let r = 8.0;
    c.line(cx - 4.0, cy - r, cx - 4.0, cy + r, 3.5, colour);
    c.line(cx + 4.0, cy - r, cx + 4.0, cy + r, 3.5, colour);
}

fn glyph_prev(c: &Canvas, cx: f64, cy: f64, colour: Rgba) {
    let r = 7.0;
    c.line(cx - 6.0, cy - r, cx - 6.0, cy + r, 2.0, colour);
    let steps = 12;
    for i in 0..steps {
        let t = i as f64 / steps as f64;
        let half = r * t;
        let x = cx - 4.0 + t * r;
        c.line(x, cy - half, x, cy + half, r / steps as f64 + 0.6, colour);
    }
}

fn glyph_next(c: &Canvas, cx: f64, cy: f64, colour: Rgba) {
    let r = 7.0;
    c.line(cx + 6.0, cy - r, cx + 6.0, cy + r, 2.0, colour);
    let steps = 12;
    for i in 0..steps {
        let t = i as f64 / steps as f64;
        let half = r * (1.0 - t);
        let x = cx - 4.0 + t * r;
        c.line(x, cy - half, x, cy + half, r / steps as f64 + 0.6, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::Image;

    fn clip(id: &str, name: &str, start: f64, end: f64, source: f64) -> Clip {
        Clip {
            id: id.into(),
            path: format!("/home/j/Videos/{name}"),
            name: name.into(),
            start,
            end,
            source_duration: source,
            speed: 1.0,
            volume: 1.0,
            thumb: None,
        }
    }

    fn three() -> Player {
        Player::new(
            "holiday",
            vec![
                clip("c1", "arrival.mp4", 0.0, 10.0, 10.0),
                clip("c2", "beach.mp4", 5.0, 25.0, 60.0),
                clip("c3", "sunset.mp4", 0.0, 30.0, 30.0),
            ],
        )
    }

    #[test]
    fn a_clips_length_is_what_survives_trimming_and_speed() {
        let mut c = clip("c1", "x.mp4", 10.0, 40.0, 60.0);
        assert_eq!(c.duration(), 30.0);
        c.speed = 2.0;
        assert_eq!(c.duration(), 15.0, "twice as fast is half as long");
        c.speed = 0.5;
        assert_eq!(c.duration(), 60.0);
        // A nonsensical speed must not divide by zero or go negative.
        c.speed = 0.0;
        assert_eq!(c.duration(), 30.0);
        c.speed = 1.0;
        c.end = 5.0;
        assert_eq!(c.duration(), 0.0, "an inverted clip is empty, not negative");
    }

    #[test]
    fn the_timeline_is_as_long_as_its_clips_add_up_to() {
        let p = three();
        assert_eq!(p.duration(), 10.0 + 20.0 + 30.0);
        assert_eq!(p.clip_start(0), 0.0);
        assert_eq!(p.clip_start(1), 10.0);
        assert_eq!(p.clip_start(2), 30.0);
    }

    #[test]
    fn a_position_maps_to_the_clip_actually_playing_there() {
        let p = three();
        assert_eq!(p.at(0.0), Some((0, 0.0)));
        assert_eq!(p.at(9.99).map(|(i, _)| i), Some(0));
        assert_eq!(
            p.at(10.0),
            Some((1, 0.0)),
            "the boundary belongs to the next clip"
        );
        assert_eq!(p.at(29.0).map(|(i, _)| i), Some(1));
        assert_eq!(p.at(30.0), Some((2, 0.0)));
        assert_eq!(p.at(59.9).map(|(i, _)| i), Some(2));
        // The very end is the end of the last clip, not past it.
        assert_eq!(p.at(60.0).map(|(i, _)| i), Some(2));
        assert_eq!(p.at(61.0), None);
    }

    #[test]
    fn an_empty_timeline_has_no_position_and_does_not_panic() {
        let p = Player::new("empty", vec![]);
        assert_eq!(p.duration(), 0.0);
        assert_eq!(p.at(0.0), None);
        assert_eq!(p.clip_start(0), 0.0);
    }

    #[test]
    fn seeking_stops_at_both_ends() {
        let mut p = three();
        p.seek(-10.0);
        assert_eq!(p.position, 0.0);
        p.seek(1000.0);
        assert_eq!(p.position, 60.0);
        p.nudge(-5.0);
        assert_eq!(p.position, 55.0);
        p.nudge(500.0);
        assert_eq!(p.position, 60.0);
    }

    #[test]
    fn marking_in_moves_the_start_into_the_source_not_the_timeline() {
        // Clip 2 already starts 5s into a 60s file. Standing 4s into that clip
        // on the timeline means 9s into the source.
        let mut p = three();
        p.select(1);
        p.seek(p.clip_start(1) + 4.0);
        let (id, at) = p.mark_in().expect("marked");
        assert_eq!(id, "c2");
        assert!((at - 9.0).abs() < 1e-9, "in-point landed at {at}");
        assert!(
            (p.clips[1].duration() - 16.0).abs() < 1e-9,
            "clip should be 16s now"
        );
    }

    #[test]
    fn marking_out_shortens_the_clip_from_the_end() {
        let mut p = three();
        p.select(0);
        p.seek(6.0);
        let (id, at) = p.mark_out().expect("marked");
        assert_eq!(id, "c1");
        assert!((at - 6.0).abs() < 1e-9);
        assert!((p.clips[0].duration() - 6.0).abs() < 1e-9);
    }

    #[test]
    fn a_cut_can_never_produce_a_clip_that_ends_before_it_starts() {
        let mut p = three();
        p.select(0);
        // Mark out at the very beginning: the clip must keep a sliver rather
        // than invert.
        p.seek(0.0);
        p.mark_out().expect("marked");
        assert!(p.clips[0].end > p.clips[0].start, "clip inverted");
        assert!(p.clips[0].duration() >= MIN_CLIP - 1e-9);

        // And marking in at the very end likewise.
        let mut p = three();
        p.select(2);
        p.seek(p.duration());
        p.mark_in();
        assert!(p.clips[2].end > p.clips[2].start, "clip inverted");
    }

    #[test]
    fn marking_out_never_runs_past_the_end_of_the_source_file() {
        let mut p = Player::new("x", vec![clip("c1", "short.mp4", 0.0, 5.0, 5.0)]);
        p.select(0);
        p.seek(5.0);
        p.mark_out();
        assert!(
            p.clips[0].end <= 5.0,
            "out-point past the source: {}",
            p.clips[0].end
        );
    }

    #[test]
    fn speed_is_accounted_for_when_marking() {
        // A clip playing at double speed: 3 seconds of timeline is 6 seconds
        // of source. Marking in must land at 6, not 3.
        let mut p = Player::new("x", vec![clip("c1", "fast.mp4", 0.0, 60.0, 60.0)]);
        p.clips[0].speed = 2.0;
        p.select(0);
        p.seek(3.0);
        let (_, at) = p.mark_in().expect("marked");
        assert!((at - 6.0).abs() < 1e-9, "landed at {at}");
    }

    #[test]
    fn timecode_shows_hours_only_when_there_are_hours() {
        assert_eq!(timecode(0.0), "0:00");
        assert_eq!(timecode(9.0), "0:09");
        assert_eq!(timecode(64.0), "1:04");
        assert_eq!(timecode(599.0), "9:59");
        assert_eq!(timecode(3600.0), "1:00:00");
        assert_eq!(timecode(3723.0), "1:02:03");
        // Nonsense in, something sensible out.
        assert_eq!(timecode(-5.0), "0:00");
    }

    #[test]
    fn the_timeline_only_appears_when_there_is_more_than_one_clip() {
        let one = Player::new("x", vec![clip("c1", "song.mp3", 0.0, 200.0, 200.0)]);
        let l = Layout::compute(&one, 900.0, 600.0);
        assert_eq!(
            l.timeline.h, 0.0,
            "one song does not need an editing surface"
        );
        assert!(l.rows.is_empty());

        let many = three();
        let l = Layout::compute(&many, 900.0, 600.0);
        assert!(l.timeline.h > 0.0);
        assert_eq!(l.rows.len(), 3);
    }

    #[test]
    fn clips_are_laid_out_in_proportion_and_never_overlap() {
        let p = three();
        let l = Layout::compute(&p, 900.0, 600.0);
        // 10 / 20 / 30 seconds: the third should be about three times the first.
        let (a, b, c) = (l.rows[0].w, l.rows[1].w, l.rows[2].w);
        assert!((b / a - 2.0).abs() < 0.15, "{a} {b} {c}");
        assert!((c / a - 3.0).abs() < 0.2, "{a} {b} {c}");
        for i in 1..l.rows.len() {
            assert!(l.rows[i].x >= l.rows[i - 1].right(), "clip {i} overlaps");
        }
        assert!(
            l.rows[2].right() <= l.timeline.right() + 0.001,
            "runs off the end"
        );
    }

    #[test]
    fn a_very_short_clip_still_gets_something_to_aim_at() {
        let p = Player::new(
            "x",
            vec![
                clip("c1", "blink.mp4", 0.0, 0.2, 0.2),
                clip("c2", "long.mp4", 0.0, 600.0, 600.0),
            ],
        );
        let l = Layout::compute(&p, 900.0, 600.0);
        assert!(
            l.rows[0].w >= 18.0,
            "a 0.2s clip became {}px wide",
            l.rows[0].w
        );
    }

    #[test]
    fn a_project_on_disk_becomes_a_timeline() {
        let doc = nous_core::json::parse(
            r#"{"version":1,"name":"holiday","clips":[
                 {"id":"c1","path":"/home/j/Videos/arrival.mp4","in":0,"out":30,"duration":90,"speed":1,"volume":1},
                 {"id":"c2","path":"/home/j/Videos/beach.mp4","in":5,"out":25,"duration":25,"speed":2,"volume":0.5}
               ]}"#,
        )
        .unwrap();
        let p = Player::from_project(&doc, |_| None);
        assert_eq!(p.project, "holiday");
        assert_eq!(p.clips.len(), 2);
        assert_eq!(
            p.clips[0].name, "arrival.mp4",
            "the name is the file, not the path"
        );
        assert_eq!(
            p.clips[0].source_duration, 90.0,
            "the source's own length was lost"
        );
        assert_eq!(p.clips[1].duration(), 10.0, "20s at double speed is 10s");
        assert_eq!(p.duration(), 40.0);
    }

    #[test]
    fn an_older_project_without_source_lengths_reads_as_untrimmed() {
        // Documents written before clips carried their source's length have
        // only an out-point. Guessing a longer file would draw a cut that was
        // never made.
        let doc = nous_core::json::parse(
            r#"{"name":"old","clips":[{"id":"c1","path":"/v/a.mp4","in":0,"out":12}]}"#,
        )
        .unwrap();
        let p = Player::from_project(&doc, |_| None);
        assert_eq!(p.clips[0].source_duration, 12.0);
        assert_eq!(
            p.clips[0].speed, 1.0,
            "a missing speed is not a stopped clip"
        );
        assert_eq!(p.clips[0].duration(), 12.0);
    }

    #[test]
    fn a_cut_lands_on_the_clip_being_watched_not_the_one_highlighted() {
        // The playhead runs on while you watch; the highlight stays where it
        // was last clicked. Cutting the highlighted clip means trimming the
        // wrong file, by an offset measured from the wrong place — here it
        // would have taken clip 1 down to nothing.
        let mut p = three();
        p.select(0);
        p.seek(12.0); // two seconds into clip 2
        let before = p.clips[0].clone();

        let (id, at) = p.mark_in().expect("marking in");
        assert_eq!(id, "c2", "cut the wrong clip");
        assert!((at - 7.0).abs() < 1e-9, "cut at the wrong place: {at}");
        assert_eq!(p.clips[0], before, "the untouched clip was changed");
        assert_eq!(p.selected, 1, "the highlight should follow the cut");
    }

    #[test]
    fn a_gesture_becomes_a_request_rather_than_an_edit() {
        let mut p = three();
        p.seek(12.0); // 2 seconds into clip 2
        let before = p.clips[1].start;

        let i = p.apply(Command::MarkIn).expect("marking in is a request");
        assert_eq!(i.cap, "media.edit");
        assert_eq!(i.args.str_or("op", ""), "trim");
        assert_eq!(
            i.args.str_or("clip", ""),
            "c2",
            "the wrong clip would be cut"
        );
        assert_eq!(i.args.str_or("project", ""), "holiday");
        // Clip 2 runs 5–25 of its source; two seconds in is source second 7.
        assert!(
            (i.args.f64_or("in", -1.0) - 7.0).abs() < 1e-9,
            "{:?}",
            i.args
        );
        assert!(
            p.clips[1].start > before,
            "the view did not move with the request"
        );
    }

    #[test]
    fn marking_out_sends_both_ends_so_the_other_one_cannot_drift() {
        let mut p = three();
        p.select(1);
        p.seek(p.clip_start(1) + 8.0);
        let i = p.apply(Command::MarkOut).expect("marking out is a request");
        assert!(
            (i.args.f64_or("in", -1.0) - 5.0).abs() < 1e-9,
            "in-point not sent: {:?}",
            i.args
        );
        assert!(
            (i.args.f64_or("out", -1.0) - 13.0).abs() < 1e-9,
            "{:?}",
            i.args
        );
    }

    #[test]
    fn seeking_asks_for_an_absolute_position_not_a_nudge() {
        // Relative seeks accumulate error and cannot express "go here". A
        // scrub bar means the second thing.
        let mut p = three();
        let i = p.apply(Command::SeekTo(33.0)).expect("seek");
        assert_eq!(i.cap, "media.control");
        assert_eq!(i.args.str_or("action", ""), "seek");
        assert_eq!(i.args.f64_or("to", -1.0), 33.0);
        assert_eq!(p.position, 33.0);
    }

    #[test]
    fn previous_restarts_the_clip_before_it_leaves_it() {
        let mut p = three();
        p.seek(p.clip_start(2) + 5.0);
        p.apply(Command::Prev);
        assert_eq!(
            p.position,
            p.clip_start(2),
            "should have gone to the head of this clip"
        );
        p.apply(Command::Prev);
        assert_eq!(
            p.position,
            p.clip_start(1),
            "a second press should step back one"
        );
        // And it stops rather than wrapping round to the end.
        p.apply(Command::Prev);
        p.apply(Command::Prev);
        assert_eq!(p.position, 0.0);
    }

    #[test]
    fn next_stops_at_the_end_instead_of_wrapping() {
        let mut p = three();
        p.seek(p.clip_start(2) + 1.0);
        p.apply(Command::Next);
        assert_eq!(
            p.position,
            p.duration(),
            "past the last clip is the end, not the start"
        );
    }

    #[test]
    fn play_pause_toggles_and_says_so() {
        let mut p = three();
        assert_eq!(p.transport, Transport::Stopped);
        let i = p.apply(Command::PlayPause).unwrap();
        assert_eq!(p.transport, Transport::Playing);
        assert_eq!(i.args.str_or("action", ""), "toggle");
        p.apply(Command::PlayPause);
        assert_eq!(p.transport, Transport::Paused);
    }

    #[test]
    fn volume_is_sent_in_the_units_the_player_counts_in() {
        let mut p = three();
        let i = p.apply(Command::SetVolume(0.4)).unwrap();
        assert_eq!(
            i.args.f64_or("level", -1.0),
            40.0,
            "mpv counts out of a hundred"
        );
        // And it cannot be pushed past either end.
        p.apply(Command::SetVolume(2.0));
        assert_eq!(p.volume, 1.0);
    }

    #[test]
    fn choosing_a_clip_is_the_views_own_business() {
        let mut p = three();
        assert_eq!(
            p.apply(Command::Select(2)),
            None,
            "highlighting asked the daemon for something"
        );
        assert_eq!(p.selected, 2);
    }

    #[test]
    fn rendering_says_it_is_happening_rather_than_looking_idle() {
        let mut p = three();
        let i = p.apply(Command::Render).unwrap();
        assert_eq!(i.cap, "media.render");
        assert_eq!(i.args.str_or("project", ""), "holiday");
        assert!(
            p.rendering.is_some(),
            "a two-minute render looks like a hang"
        );
    }

    #[test]
    fn gestures_on_an_empty_timeline_ask_for_nothing() {
        let mut p = Player::new("x", vec![]);
        for cmd in [
            Command::Prev,
            Command::Next,
            Command::MarkIn,
            Command::MarkOut,
        ] {
            assert_eq!(p.apply(cmd), None, "{cmd:?} on an empty timeline");
        }
    }

    #[test]
    fn the_playhead_lands_on_the_clip_it_is_actually_playing() {
        // A blink between two long takes. The blink is given a minimum width,
        // so pixels along the strip stop being proportional to seconds — which
        // is exactly when reading the playhead off a fraction of the whole
        // strip goes wrong.
        let mut p = Player::new(
            "x",
            vec![
                clip("c1", "a.mp4", 0.0, 60.0, 60.0),
                clip("c2", "blink.mp4", 0.0, 0.5, 0.5),
                clip("c3", "b.mp4", 0.0, 60.0, 60.0),
            ],
        );
        let l = Layout::compute(&p, 900.0, 600.0);

        // At the head of the third clip, the playhead is at that clip's edge.
        p.seek(p.clip_start(2));
        let x = l.playhead_x(&p).expect("a timeline has a playhead");
        assert!(
            (x - l.rows[2].x).abs() < 0.001,
            "{x} is not the start of clip 3 at {}",
            l.rows[2].x
        );

        // Reading it off a fraction of the whole strip does not merely drift:
        // it points at the wrong clip. Without this the test above could pass
        // on a timeline where both readings happen to agree.
        let span = l.rows[2].right() - l.rows[0].x;
        let naive = l.rows[0].x + span * (p.position / p.duration());
        let mid_y = l.rows[1].y + l.rows[1].h / 2.0;
        assert_eq!(
            l.clip_at(naive, mid_y),
            Some(1),
            "clip 3 is playing; the strip-fraction reading should be pointing at the blink"
        );

        // Halfway through the first clip is halfway across its row.
        p.seek(30.0);
        let x = l.playhead_x(&p).expect("playhead");
        assert!((x - (l.rows[0].x + l.rows[0].w / 2.0)).abs() < 0.001, "{x}");
    }

    #[test]
    fn the_playhead_stops_at_the_end_of_the_material() {
        let mut p = three();
        let l = Layout::compute(&p, 900.0, 600.0);
        p.seek(p.duration());
        let x = l.playhead_x(&p).expect("playhead");
        let last = l.rows[l.rows.len() - 1];
        assert!(
            x <= last.right() + 0.001 && x >= last.x,
            "{x} is off the end"
        );
    }

    #[test]
    fn a_timeline_with_no_clips_has_no_playhead() {
        let p = Player::new("x", vec![]);
        let l = Layout::compute(&p, 900.0, 600.0);
        assert_eq!(l.playhead_x(&p), None);
    }

    #[test]
    fn stacked_layout_leaves_every_part_room_and_in_order() {
        let p = three();
        let l = Layout::compute(&p, 900.0, 600.0);
        assert!(l.stage.h > 0.0, "no room left for the picture");
        assert!(l.stage.bottom() <= l.scrub.y + 0.001);
        assert!(l.scrub.bottom() <= l.transport.y + 0.001);
        assert!(l.transport.bottom() <= l.timeline.y + 0.001);
        assert!(l.timeline.bottom() <= l.panel.bottom() + 0.001);
    }

    #[test]
    fn a_window_too_short_for_everything_does_not_produce_a_negative_stage() {
        let p = three();
        let l = Layout::compute(&p, 900.0, 80.0);
        assert!(
            l.stage.h >= 0.0,
            "stage height went negative: {}",
            l.stage.h
        );
    }

    #[test]
    fn clicking_the_scrub_bar_seeks_proportionally() {
        let p = three();
        let l = Layout::compute(&p, 900.0, 600.0);
        assert!((l.scrub_fraction(l.scrub.x) - 0.0).abs() < 1e-9);
        assert!((l.scrub_fraction(l.scrub.x + l.scrub.w) - 1.0).abs() < 1e-9);
        assert!((l.scrub_fraction(l.scrub.x + l.scrub.w / 2.0) - 0.5).abs() < 1e-9);
        // Outside the bar clamps rather than seeking off the end.
        assert_eq!(l.scrub_fraction(l.scrub.x - 500.0), 0.0);
        assert_eq!(l.scrub_fraction(l.scrub.right() + 500.0), 1.0);
    }

    #[test]
    fn clicking_a_clip_finds_that_clip() {
        let p = three();
        let l = Layout::compute(&p, 900.0, 600.0);
        for i in 0..3 {
            let r = l.rows[i];
            assert_eq!(l.clip_at(r.x + 2.0, r.y + 2.0), Some(i));
        }
        assert_eq!(l.clip_at(l.stage.x + 5.0, l.stage.y + 5.0), None);
    }

    #[test]
    fn the_player_draws_a_stage_a_scrub_and_a_timeline() {
        let theme = Theme::dark();
        let mut p = three();
        p.position = 25.0;
        p.transport = Transport::Playing;
        let img = Image::new(900, 600).unwrap();
        let c = img.canvas();
        let l = Layout::compute(&p, 900.0, 600.0);
        render(&c, &mut p, &theme, &l);

        assert!(img.variety(l.transport) > 5, "the transport row is blank");
        assert!(img.variety(l.timeline) > 5, "the timeline is blank");
        // The played part of the scrub bar is the accent; the rest is not.
        let y = (l.scrub.y + l.scrub.h / 2.0) as i32;
        let early = img.pixel((l.scrub.x + 10.0) as i32, y);
        let late = img.pixel((l.scrub.right() - 10.0) as i32, y);
        assert_ne!(early, late, "the scrub bar shows no progress");
        assert!(
            early.0 > 150 && early.1 > 100,
            "played part is not the accent: {early:?}"
        );
    }

    #[test]
    fn a_trimmed_clip_shows_which_part_of_the_file_survived() {
        // Two clips of the same finished length, so their rows are the same
        // width, cut from different parts of their sources: the first keeps
        // the front of its file, the second the back. The ribbon under each
        // row is what says so — the rows themselves cannot, because a row is
        // as wide as what the cut kept, not as wide as the file.
        let theme = Theme::dark();
        let mut p = Player::new(
            "x",
            vec![
                clip("c1", "front.mp4", 0.0, 30.0, 60.0),
                clip("c2", "back.mp4", 30.0, 60.0, 60.0),
            ],
        );
        let img = Image::new(900, 600).unwrap();
        let l = Layout::compute(&p, 900.0, 600.0);
        render(&img.canvas(), &mut p, &theme, &l);

        // Sampled at the top of the ribbon, clear of the selection ring, which
        // runs along the very edge of the row in the same accent colour.
        let sample = |row: Rect, f: f64| {
            img.pixel((row.x + row.w * f) as i32, (row.bottom() - RIBBON_H) as i32)
        };
        // Front-trimmed: lit at the head, unlit at the tail. Back-trimmed: the
        // other way round. Each clip is compared against itself, because the
        // selected clip's ribbon is lit in the accent and the rest in a
        // quieter colour.
        assert_ne!(
            sample(l.rows[0], 0.25),
            sample(l.rows[0], 0.75),
            "the first clip's ribbon is a solid bar, so it says nothing about the cut"
        );
        assert_ne!(
            sample(l.rows[1], 0.25),
            sample(l.rows[1], 0.75),
            "the second clip's ribbon is a solid bar"
        );
        // The two dropped halves are drawn the same way, so the ribbons really
        // are reading opposite ends of their files.
        assert_eq!(
            sample(l.rows[0], 0.75),
            sample(l.rows[1], 0.25),
            "the two cuts are not drawn the other way round from each other"
        );
    }

    #[test]
    fn a_clip_with_a_frame_shows_it_behind_the_name() {
        // The row's ground is the frame. Without it the timeline is a strip of
        // flat rectangles, which is what it looked like before.
        let dir = std::env::temp_dir().join("nous-player-frame-test");
        let _ = std::fs::create_dir_all(&dir);
        let frame = dir.join("f.png");
        let src = Image::new(64, 64).unwrap();
        let sc = src.canvas();
        for y in 0..64 {
            let t = y as f64 / 63.0;
            sc.fill_rect(
                Rect::new(0.0, y as f64, 64.0, 1.0),
                Rgba::rgb(20, 40, 60).mix(Rgba::rgb(230, 180, 90), t),
            );
        }
        src.write_png(frame.to_str().unwrap()).unwrap();

        let theme = Theme::dark();
        let clips = vec![
            clip("c1", "a.mp4", 0.0, 10.0, 10.0),
            clip("c2", "b.mp4", 0.0, 10.0, 10.0),
        ];
        let mut bare = Player::new("x", clips.clone());
        let mut with = Player::new("x", clips);
        with.clips[1].thumb = Some(frame.to_str().unwrap().to_string());

        let l = Layout::compute(&bare, 900.0, 600.0);
        let plain = Image::new(900, 600).unwrap();
        render(&plain.canvas(), &mut bare, &theme, &l);
        let framed = Image::new(900, 600).unwrap();
        render(&framed.canvas(), &mut with, &theme, &l);

        // Sampled on the right of the row, clear of the caption, and above the
        // trim ribbon: with no frame that is flat surface colour.
        let row = l.rows[1];
        let x = (row.right() - 8.0) as i32;
        let (hi, lo) = ((row.y + 6.0) as i32, (row.bottom() - 10.0) as i32);
        assert_ne!(
            framed.pixel(x, hi),
            plain.pixel(x, hi),
            "the frame is not drawn: the row looks the same with and without one"
        );
        assert_ne!(
            framed.pixel(x, hi),
            framed.pixel(x, lo),
            "the row is a flat tint rather than the picture"
        );
        assert_eq!(
            plain.pixel(x, hi),
            plain.pixel(x, lo),
            "a clip with no frame should be a flat fill, so the check above proves nothing"
        );
        let _ = std::fs::remove_file(&frame);
    }

    #[test]
    fn the_stage_is_black_whatever_the_theme() {
        // A picture is judged against black. In the light theme especially,
        // the surround must not be the page colour.
        for theme in [Theme::dark(), Theme::light()] {
            let mut p = three();
            let img = Image::new(600, 500).unwrap();
            let c = img.canvas();
            let l = Layout::compute(&p, 600.0, 500.0);
            render(&c, &mut p, &theme, &l);
            let px = img.pixel(4, 4);
            assert!(
                px.0 < 20 && px.1 < 20 && px.2 < 20,
                "{:?} stage is not black: {px:?}",
                theme.mode
            );
        }
    }
}
