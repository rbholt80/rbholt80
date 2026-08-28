//! Playing things: the surface for a library, not a cutting room.
//!
//! Separate from the editor because a play queue and an edit timeline are not
//! the same list wearing different clothes. Clips on a timeline are measured in
//! the finished piece and butt against each other; a queue is an ordered set of
//! whole files, any of which may be next. Drawing one as the other means either
//! a playlist whose rows are two pixels wide because the track is short, or a
//! timeline that has forgotten how long its clips are.
//!
//! Everything drawn here is applied from a `media.state` report. The view holds
//! no opinion about where playback is; it holds the last thing the player said,
//! and says how old that is.

use crate::draw::{Canvas, Picture, Rect, Rgba};
use crate::player::{
    glyph_next, glyph_pause, glyph_play, glyph_prev, timecode, SCRUB_H, TRANSPORT_H,
};
use crate::theme::{Metrics, Theme};
use nous_core::json::{json_obj, Json};
use std::collections::HashMap;

/// What a file is, which decides what to draw when there is no picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
}

/// One entry in the queue.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub path: String,
    /// What to call it: the file's own title where it has one, its filename
    /// where it does not.
    pub title: String,
    pub artist: String,
    pub duration: f64,
    pub kind: Kind,
    /// Cover art or a poster frame, cached by the daemon.
    pub art: Option<String>,
}

impl Track {
    pub fn from_path(path: &str) -> Track {
        let name = file_name(path);
        Track {
            path: path.to_string(),
            title: name,
            artist: String::new(),
            duration: 0.0,
            kind: kind_of(path),
            art: None,
        }
    }
}

fn file_name(path: &str) -> String {
    path.rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// Audio or video, by extension.
///
/// The distinction earns its keep in one place: an audio file has no picture,
/// so a stage that assumes there is one shows a black rectangle for the length
/// of the album.
pub fn kind_of(path: &str) -> Kind {
    const AUDIO: [&str; 10] = [
        "mp3", "flac", "ogg", "oga", "opus", "m4a", "wav", "aac", "wma", "aiff",
    ];
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if AUDIO.contains(&ext.as_str()) {
        Kind::Audio
    } else {
        Kind::Video
    }
}

/// A selectable audio or subtitle stream, as the player reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub id: String,
    /// "English", "Director's commentary" — whatever the file says.
    pub label: String,
    pub selected: bool,
}

/// The player, as last reported.
pub struct Queue {
    pub tracks: Vec<Track>,
    /// Which entry is playing. `None` when the queue is empty.
    pub current: Option<usize>,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    /// Out of one hundred, as the player counts it.
    pub volume: f64,
    pub speed: f64,
    pub subtitles: Vec<Stream>,
    pub audio: Vec<Stream>,
    /// Which entry the keyboard is on, which is not necessarily the one
    /// playing: you scroll a queue looking for the next thing while the current
    /// thing keeps playing.
    pub selected: usize,
    pub scroll: f64,
    /// Set while the position is the interface's own, because someone is
    /// dragging the scrub bar. Reports are ignored until they catch up.
    pub scrubbing: bool,
    cache: HashMap<String, Option<Picture>>,
}

impl Default for Queue {
    fn default() -> Queue {
        Queue::new(Vec::new())
    }
}

impl Queue {
    pub fn new(tracks: Vec<Track>) -> Queue {
        Queue {
            current: if tracks.is_empty() { None } else { Some(0) },
            tracks,
            position: 0.0,
            duration: 0.0,
            paused: false,
            volume: 100.0,
            speed: 1.0,
            subtitles: Vec::new(),
            audio: Vec::new(),
            selected: 0,
            scroll: 0.0,
            scrubbing: false,
            cache: HashMap::new(),
        }
    }

    pub fn playing(&self) -> Option<&Track> {
        self.current.and_then(|i| self.tracks.get(i))
    }

    /// Take what the player said. This is the only way facts get in.
    ///
    /// A report that arrives while the scrub bar is being dragged keeps its
    /// tracks and its queue but not its position: the player is still seeking
    /// towards where the finger already is, and letting it answer would drag
    /// the handle backwards under the hand holding it.
    pub fn apply(&mut self, report: &Json, art: impl Fn(&str) -> Option<String>) {
        let playing = report.bool_or("playing", false);
        if !playing {
            self.current = None;
            self.position = 0.0;
            self.duration = 0.0;
            self.paused = false;
            self.subtitles.clear();
            self.audio.clear();
            return;
        }
        self.paused = report.bool_or("paused", false);
        self.duration = report.f64_or("duration", 0.0);
        self.volume = report.f64_or("volume", self.volume);
        self.speed = {
            let s = report.f64_or("speed", 1.0);
            if s > 0.0 {
                s
            } else {
                1.0
            }
        };
        if !self.scrubbing {
            self.position = report.f64_or("position", 0.0);
        }

        let queue = report.arr_or_empty("queue");
        if !queue.is_empty() {
            let title = report.str_or("title", "");
            self.tracks = queue
                .iter()
                .map(|e| {
                    let path = e.str_or("filename", "").to_string();
                    let mut t = Track::from_path(&path);
                    // The player names only the file it is on; the rest of the
                    // queue is named by its filename until it gets there.
                    if e.bool_or("current", false) && !title.is_empty() {
                        t.title = title.to_string();
                    } else if let Some(n) = e.get("title").and_then(|v| v.as_str()) {
                        if !n.is_empty() {
                            t.title = n.to_string();
                        }
                    }
                    t.art = art(&path);
                    t
                })
                .collect();
            let pos = report.f64_or("queue_pos", 0.0);
            self.current = if pos >= 0.0 && (pos as usize) < self.tracks.len() {
                Some(pos as usize)
            } else {
                Some(0)
            };
        } else {
            // A player given one file reports no queue at all.
            let path = report.str_or("path", "").to_string();
            let mut t = Track::from_path(&path);
            let title = report.str_or("title", "");
            if !title.is_empty() {
                t.title = title.to_string();
            }
            t.duration = self.duration;
            t.art = art(&path);
            self.tracks = vec![t];
            self.current = Some(0);
        }
        if let Some(i) = self.current {
            if let Some(t) = self.tracks.get_mut(i) {
                t.duration = self.duration;
            }
        }
        self.selected = self.selected.min(self.tracks.len().saturating_sub(1));
        (self.subtitles, self.audio) = read_streams(report);
    }

    /// How far through, as a fraction. Zero rather than a division by zero when
    /// the length is not known yet — which is the first second of every file.
    pub fn progress(&self) -> f64 {
        if self.duration > 0.0 {
            (self.position / self.duration).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    pub fn move_selection(&mut self, delta: i64) {
        if self.tracks.is_empty() {
            return;
        }
        let last = self.tracks.len() as i64 - 1;
        self.selected = (self.selected as i64 + delta).clamp(0, last) as usize;
    }

    fn picture(&mut self, path: &str) -> Option<&Picture> {
        self.cache
            .entry(path.to_string())
            .or_insert_with(|| Picture::load(path).ok())
            .as_ref()
    }
}

/// Pull the selectable audio and subtitle streams out of a report.
///
/// The player reports one flat list with a `type` on each entry, so this is a
/// split rather than two lookups.
fn read_streams(report: &Json) -> (Vec<Stream>, Vec<Stream>) {
    let mut subs = Vec::new();
    let mut audio = Vec::new();
    for t in report.arr_or_empty("tracks") {
        let kind = t.str_or("type", "");
        let id = match t.get("id").and_then(|v| v.as_f64()) {
            Some(n) => format!("{}", n as i64),
            None => continue,
        };
        let label = {
            let title = t.str_or("title", "");
            let lang = t.str_or("lang", "");
            match (title.is_empty(), lang.is_empty()) {
                (false, false) => format!("{} ({})", title, lang),
                (false, true) => title.to_string(),
                (true, false) => lang.to_string(),
                // A stream with neither is still choosable, and calling it
                // nothing would leave an empty row you cannot aim at.
                (true, true) => format!("track {}", id),
            }
        };
        let s = Stream {
            id,
            label,
            selected: t.bool_or("selected", false),
        };
        match kind {
            "sub" => subs.push(s),
            "audio" => audio.push(s),
            _ => {}
        }
    }
    (subs, audio)
}

// --- what a gesture asks for ----------------------------------------------

pub use crate::player::Intent;

#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    PlayPause,
    Next,
    Previous,
    /// Play the entry the keyboard is on.
    PlaySelected,
    /// Add a file to the end of the queue instead of interrupting.
    Enqueue(String),
    SeekTo(f64),
    Nudge(f64),
    SetVolume(f64),
    SetSpeed(f64),
    /// By track id, or `None` to turn them off.
    Subtitles(Option<String>),
    AudioTrack(String),
    Fullscreen,
    Stop,
}

impl Queue {
    /// Say what a gesture means, in capability terms. Nothing is done here.
    pub fn act(&mut self, a: Act) -> Option<Intent> {
        let control = |action: &str, args: Vec<(&str, Json)>| {
            let mut o = json_obj([("action", action.into())]);
            for (k, v) in args {
                o.set(k, v);
            }
            Some(Intent {
                cap: "media.control",
                args: o,
            })
        };
        match a {
            Act::PlayPause => {
                self.paused = !self.paused;
                control("toggle", vec![])
            }
            Act::Next => control("next", vec![]),
            Act::Previous => control("previous", vec![]),
            Act::PlaySelected => {
                if self.tracks.is_empty() {
                    return None;
                }
                let i = self.selected;
                self.current = Some(i);
                control("goto", vec![("index", (i as f64).into())])
            }
            Act::Enqueue(path) => Some(Intent {
                cap: "media.play",
                args: json_obj([("path", path.into()), ("queue", Json::Bool(true))]),
            }),
            Act::SeekTo(t) => {
                self.position = t.clamp(0.0, self.duration.max(t));
                control("seek", vec![("to", self.position.into())])
            }
            Act::Nudge(dt) => {
                self.position = (self.position + dt).max(0.0);
                // Relative, because that is what it is: asking for an absolute
                // position computed from a report that may be a moment old
                // would make each nudge inherit that report's staleness.
                control("seek", vec![("seconds", dt.into())])
            }
            Act::SetVolume(v) => {
                self.volume = v.clamp(0.0, 100.0);
                control("volume", vec![("level", self.volume.into())])
            }
            Act::SetSpeed(s) => {
                self.speed = s.clamp(0.25, 4.0);
                control("speed", vec![("value", self.speed.into())])
            }
            Act::Subtitles(id) => {
                let want = id.unwrap_or_else(|| "no".to_string());
                for s in &mut self.subtitles {
                    s.selected = s.id == want;
                }
                control("subtitles", vec![("track", want.into())])
            }
            Act::AudioTrack(id) => {
                for s in &mut self.audio {
                    s.selected = s.id == id;
                }
                control("audio_track", vec![("track", id.into())])
            }
            Act::Fullscreen => control("fullscreen", vec![]),
            Act::Stop => {
                self.current = None;
                control("stop", vec![])
            }
        }
    }
}

// --- layout ----------------------------------------------------------------

/// How wide the queue gets when there is room for it beside the picture.
const QUEUE_W: f64 = 268.0;
/// Below this the queue goes away rather than squeezing the picture to nothing.
const QUEUE_MIN_STAGE: f64 = 420.0;
const ROW_H: f64 = 46.0;
const STREAMS_H: f64 = 30.0;

pub struct Layout {
    pub panel: Rect,
    pub stage: Rect,
    /// Where the audio and subtitle choices go. Zero-height when the file
    /// offers none, which is every music file and most home video.
    pub streams: Rect,
    pub scrub: Rect,
    pub transport: Rect,
    /// The queue list, empty when the window is too narrow to hold one.
    pub queue: Rect,
    pub rows: Vec<Rect>,
}

impl Layout {
    pub fn compute(q: &Queue, width: f64, height: f64) -> Layout {
        let pad = Metrics::PAD;
        let has_streams = !q.subtitles.is_empty() || !q.audio.is_empty();
        let streams_h = if has_streams { STREAMS_H } else { 0.0 };
        let bottom = SCRUB_H + TRANSPORT_H + streams_h + pad;
        let body_h = (height - bottom).max(0.0);

        // The queue sits beside the picture, not under it: a video is wide and
        // a list is tall, and stacking them wastes the shape of both.
        let show_queue = width - QUEUE_W - pad * 2.0 >= QUEUE_MIN_STAGE && !q.tracks.is_empty();
        let stage_w = if show_queue {
            width - QUEUE_W - pad
        } else {
            width
        };
        let stage = Rect::new(0.0, 0.0, stage_w, body_h);
        let queue = if show_queue {
            Rect::new(stage_w + pad, pad, QUEUE_W - pad * 2.0, body_h - pad * 2.0)
        } else {
            Rect::new(width, pad, 0.0, 0.0)
        };

        let mut rows = Vec::new();
        if queue.w > 0.0 {
            let mut y = queue.y - q.scroll;
            for _ in &q.tracks {
                rows.push(Rect::new(queue.x, y, queue.w, ROW_H - 4.0));
                y += ROW_H;
            }
        }

        let streams_y = body_h;
        let scrub_y = streams_y + streams_h;
        let transport_y = scrub_y + SCRUB_H;
        Layout {
            panel: Rect::new(0.0, 0.0, width, height),
            stage,
            streams: Rect::new(pad, streams_y, (width - pad * 2.0).max(0.0), streams_h),
            scrub: Rect::new(pad, scrub_y, (width - pad * 2.0).max(0.0), SCRUB_H),
            transport: Rect::new(pad, transport_y, (width - pad * 2.0).max(0.0), TRANSPORT_H),
            queue,
            rows,
        }
    }

    pub fn scrub_fraction(&self, x: f64) -> f64 {
        if self.scrub.w <= 0.0 {
            return 0.0;
        }
        ((x - self.scrub.x) / self.scrub.w).clamp(0.0, 1.0)
    }

    /// Which queue entry is under a point, ignoring rows scrolled out of sight.
    pub fn row_at(&self, x: f64, y: f64) -> Option<usize> {
        self.rows.iter().position(|r| {
            r.contains(x, y) && r.bottom() > self.queue.y && r.y < self.queue.bottom()
        })
    }

    /// How far the list can be scrolled before it runs out of entries.
    pub fn max_scroll(&self, q: &Queue) -> f64 {
        let content = q.tracks.len() as f64 * ROW_H;
        (content - self.queue.h).max(0.0)
    }
}

// --- drawing ---------------------------------------------------------------

pub fn render(c: &Canvas, q: &mut Queue, theme: &Theme, layout: &Layout) {
    c.fill_rect(layout.panel, theme.backdrop_opaque);
    draw_stage(c, q, theme, layout);
    if layout.queue.w > 0.0 {
        draw_queue(c, q, theme, layout);
    }
    if layout.streams.h > 0.0 {
        draw_streams(c, q, theme, layout);
    }
    draw_scrub(c, q, theme, layout);
    draw_transport(c, q, theme, layout);
}

/// The picture, or what stands in for one.
///
/// Video plays into this rectangle from outside — the player draws its own
/// pixels into the window, and this leaves black behind them. Audio has nothing
/// to draw, so it gets the cover and the name of what is playing: a music
/// player showing a black rectangle for forty minutes is not showing anything.
fn draw_stage(c: &Canvas, q: &mut Queue, theme: &Theme, layout: &Layout) {
    let s = layout.stage;
    if s.h <= 0.0 || s.w <= 0.0 {
        return;
    }
    c.fill_rect(s, Rgba::rgb(0, 0, 0));

    let Some(track) = q.playing().cloned() else {
        let msg = "nothing playing";
        let f = theme.title_font();
        let (w, h) = c.measure(msg, &f, None);
        c.text(
            msg,
            s.x + (s.w - w) / 2.0,
            s.y + (s.h - h) / 2.0,
            &f,
            theme.text_faint,
            None,
        );
        return;
    };

    if track.kind == Kind::Video {
        // The player owns these pixels. Drawing a poster frame here would show
        // through wherever the video is letterboxed and read as a fault.
        return;
    }

    // Cover art, as large a square as the stage allows.
    let side = (s.h * 0.56).min(s.w * 0.5);
    let art = Rect::new(
        s.x + (s.w - side) / 2.0,
        s.y + s.h * 0.5 - side * 0.72,
        side,
        side,
    );
    let mut drew = false;
    if let Some(path) = track.art.clone() {
        if let Some(pic) = q.picture(&path) {
            c.picture_rounded(pic, art, Metrics::RADIUS_SMALL);
            drew = true;
        }
    }
    if !drew {
        // A record, not a broken-image box: a filled disc with a hole reads as
        // "music" at any size and needs no glyph from a font.
        c.fill_rounded(art, Metrics::RADIUS_SMALL, theme.surface);
        let (cx, cy) = (art.x + art.w / 2.0, art.y + art.h / 2.0);
        c.fill_circle(cx, cy, art.w * 0.34, theme.surface_active);
        c.fill_circle(cx, cy, art.w * 0.08, Rgba::rgb(0, 0, 0));
    }

    // The name under it, and the artist under that.
    let title_f = theme.title_font();
    let max = s.w * 0.8;
    let (tw, th) = c.measure(&track.title, &title_f, Some(max));
    let ty = art.bottom() + 22.0;
    c.text(
        &track.title,
        s.x + (s.w - tw) / 2.0,
        ty,
        &title_f,
        theme.text,
        Some(max),
    );
    if !track.artist.is_empty() {
        let f = theme.body_font();
        let (aw, _) = c.measure(&track.artist, &f, Some(max));
        c.text(
            &track.artist,
            s.x + (s.w - aw) / 2.0,
            ty + th + 6.0,
            &f,
            theme.text_dim,
            Some(max),
        );
    }
}

fn draw_queue(c: &Canvas, q: &mut Queue, theme: &Theme, layout: &Layout) {
    let box_ = layout.queue;
    c.clip_rect(box_);
    let small = theme.small_font();
    let body = theme.body_font();
    for (i, row) in layout.rows.iter().enumerate() {
        if row.bottom() < box_.y || row.y > box_.bottom() {
            continue;
        }
        let Some(t) = q.tracks.get(i) else { continue };
        let playing = q.current == Some(i);
        if playing {
            c.fill_rounded(*row, Metrics::RADIUS_SMALL / 2.0, theme.surface_active);
        } else if q.selected == i {
            c.fill_rounded(*row, Metrics::RADIUS_SMALL / 2.0, theme.surface);
        }
        // A bar in the accent on whatever is playing, so the eye finds it
        // without reading a single title.
        if playing {
            c.fill_rounded(
                Rect::new(row.x, row.y + 6.0, 3.0, row.h - 12.0),
                1.5,
                theme.voice,
            );
        }
        let text_x = row.x + 12.0;
        let width = (row.w - 24.0 - 44.0).max(10.0);
        c.text(
            &t.title,
            text_x,
            row.y + 6.0,
            &body,
            if playing { theme.text } else { theme.text_dim },
            Some(width),
        );
        if t.duration > 0.0 {
            let d = timecode(t.duration);
            let (dw, _) = c.measure(&d, &small, None);
            c.text(
                &d,
                row.right() - dw - 10.0,
                row.y + 8.0,
                &small,
                theme.text_faint,
                None,
            );
        }
    }
    c.restore();
}

/// The audio and subtitle choices, when the file has any.
fn draw_streams(c: &Canvas, q: &Queue, theme: &Theme, layout: &Layout) {
    let r = layout.streams;
    let small = theme.small_font();
    let mut x = r.x;
    // Which stream is on is the only fact this row carries, so the difference
    // between chosen and not is the accent against the ordinary surface — not
    // two shades of the same grey, which is what it was and which reads as one
    // row of identical chips in either theme.
    let chip = |c: &Canvas, label: &str, on: bool, x: &mut f64| {
        let (w, h) = c.measure(label, &small, None);
        let box_ = Rect::new(*x, r.y + (r.h - h - 8.0) / 2.0, w + 18.0, h + 8.0);
        c.fill_rounded(
            box_,
            box_.h / 2.0,
            if on { theme.voice } else { theme.surface },
        );
        c.text(
            label,
            box_.x + 9.0,
            box_.y + 4.0,
            &small,
            if on {
                theme.backdrop_opaque
            } else {
                theme.text_dim
            },
            None,
        );
        *x = box_.right() + 6.0;
    };
    if !q.audio.is_empty() {
        c.text("AUDIO", x, r.y + 9.0, &small, theme.text_faint, None);
        let (lw, _) = c.measure("AUDIO", &small, None);
        x += lw + 8.0;
        for s in &q.audio {
            chip(c, &s.label, s.selected, &mut x);
        }
        x += 10.0;
    }
    if !q.subtitles.is_empty() {
        c.text("SUBTITLES", x, r.y + 9.0, &small, theme.text_faint, None);
        let (lw, _) = c.measure("SUBTITLES", &small, None);
        x += lw + 8.0;
        for s in &q.subtitles {
            chip(c, &s.label, s.selected, &mut x);
        }
    }
}

/// A speaker, drawn rather than lettered, with as many arcs as there is volume.
///
/// Silence draws none, which is the state most worth recognising at a glance.
fn glyph_speaker(c: &Canvas, cx: f64, cy: f64, level: f64, colour: Rgba) {
    // The cone: a stack of horizontal lines widening to the right, which needs
    // no polygon primitive.
    let steps = 9;
    for i in 0..steps {
        let t = i as f64 / (steps - 1) as f64;
        let half = 2.0 + t * 4.5;
        let x = cx - 5.0 + t * 5.0;
        c.line(x, cy - half, x, cy + half, 1.4, colour);
    }
    // Waves, one per third of the way up.
    let arcs = (level * 3.0).ceil() as i32;
    for i in 0..arcs.min(3) {
        let r = 3.0 + i as f64 * 2.6;
        let x = cx + 2.0;
        c.line(x + r, cy - r * 0.62, x + r, cy + r * 0.62, 1.2, colour);
    }
}

fn draw_scrub(c: &Canvas, q: &Queue, theme: &Theme, layout: &Layout) {
    let s = layout.scrub;
    let track = Rect::new(s.x, s.y + s.h / 2.0 - 2.0, s.w, 4.0);
    c.fill_rounded(track, 2.0, theme.surface_active);
    let at = q.progress();
    if at > 0.0 {
        c.fill_rounded(
            Rect::new(track.x, track.y, track.w * at, track.h),
            2.0,
            theme.voice,
        );
    }
    c.fill_circle(
        track.x + track.w * at,
        track.y + track.h / 2.0,
        6.0,
        theme.voice,
    );
}

fn draw_transport(c: &Canvas, q: &Queue, theme: &Theme, layout: &Layout) {
    let t = layout.transport;
    let small = theme.small_font();
    let cy = t.y + t.h / 2.0;

    let clock = format!("{} / {}", timecode(q.position), timecode(q.duration));
    let (_, ch) = c.measure(&clock, &theme.font_mono, None);
    c.text(
        &clock,
        t.x,
        cy - ch / 2.0,
        &theme.font_mono,
        theme.text,
        None,
    );

    let mid = t.x + t.w / 2.0;
    glyph_prev(c, mid - 44.0, cy, theme.text);
    if q.playing().is_some() && !q.paused {
        glyph_pause(c, mid, cy, theme.voice);
    } else {
        glyph_play(c, mid, cy, theme.voice);
    }
    glyph_next(c, mid + 44.0, cy, theme.text);

    // Right: speed when it is not one, then the volume.
    let mut right = t.right();
    let bar_w = 76.0;
    let bar = Rect::new(right - bar_w, cy - 2.0, bar_w, 4.0);
    c.fill_rounded(bar, 2.0, theme.surface_active);
    let level = (q.volume / 100.0).clamp(0.0, 1.0);
    c.fill_rounded(
        Rect::new(bar.x, bar.y, bar.w * level, bar.h),
        2.0,
        theme.text,
    );
    c.fill_circle(bar.x + bar.w * level, cy, 4.5, theme.text);
    // A bare rule beside the clock is not recognisable as a volume control; it
    // reads as a stray line. The speaker says what the line is for.
    glyph_speaker(c, bar.x - 16.0, cy, level, theme.text_dim);
    right = bar.x - 30.0;
    if (q.speed - 1.0).abs() > 0.001 {
        let label = format!("{:.2}×", q.speed);
        let (w, h) = c.measure(&label, &small, None);
        c.text(&label, right - w, cy - h / 2.0, &small, theme.voice, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::Image;
    use nous_core::json::parse;

    fn report(json: &str) -> Json {
        parse(json).expect("test report parses")
    }

    fn playing_one(path: &str) -> Json {
        report(&format!(
            r#"{{"playing":true,"paused":false,"path":"{path}","title":"",
                "position":12.0,"duration":180.0,"volume":80,"speed":1.0}}"#
        ))
    }

    #[test]
    fn a_report_is_the_only_way_facts_get_in() {
        let mut q = Queue::default();
        assert!(q.playing().is_none());
        q.apply(&playing_one("/home/j/Music/song.mp3"), |_| None);
        assert_eq!(q.position, 12.0);
        assert_eq!(q.duration, 180.0);
        assert_eq!(q.volume, 80.0);
        assert_eq!(q.playing().unwrap().title, "song.mp3");
        assert_eq!(q.playing().unwrap().kind, Kind::Audio);
    }

    #[test]
    fn a_player_with_nothing_loaded_empties_the_surface() {
        let mut q = Queue::default();
        q.apply(&playing_one("/m/a.mp3"), |_| None);
        q.apply(&report(r#"{"playing":false}"#), |_| None);
        assert!(q.playing().is_none(), "still showing a track that stopped");
        assert_eq!(q.position, 0.0);
        assert_eq!(q.duration, 0.0);
    }

    #[test]
    fn a_report_arriving_mid_drag_does_not_pull_the_handle_back() {
        // The player is still seeking towards where the finger already is, so
        // its answer is about where the playhead used to be. Taking it would
        // make the handle jump backwards under the hand holding it.
        let mut q = Queue::default();
        q.apply(&playing_one("/m/a.mp3"), |_| None);
        q.scrubbing = true;
        q.act(Act::SeekTo(150.0));
        assert_eq!(q.position, 150.0);
        q.apply(&playing_one("/m/a.mp3"), |_| None); // still says 12.0
        assert_eq!(q.position, 150.0, "the drag was undone by a stale report");
        // Once the drag ends, the player is the authority again.
        q.scrubbing = false;
        q.apply(&playing_one("/m/a.mp3"), |_| None);
        assert_eq!(q.position, 12.0);
    }

    #[test]
    fn a_queue_of_several_keeps_its_order_and_knows_which_is_playing() {
        let mut q = Queue::default();
        q.apply(
            &report(
                r#"{"playing":true,"path":"/m/b.mp3","title":"The Second One",
                    "position":5,"duration":100,"volume":70,"speed":1,
                    "queue":[{"filename":"/m/a.mp3"},
                             {"filename":"/m/b.mp3","current":true},
                             {"filename":"/m/c.mp3"}],
                    "queue_pos":1}"#,
            ),
            |_| None,
        );
        assert_eq!(q.tracks.len(), 3);
        assert_eq!(q.current, Some(1));
        assert_eq!(
            q.tracks[1].title, "The Second One",
            "the playing entry keeps its real name"
        );
        assert_eq!(
            q.tracks[0].title, "a.mp3",
            "an entry not yet reached is named by its file"
        );
    }

    #[test]
    fn a_queue_position_past_the_end_does_not_index_off_it() {
        // A report can arrive describing a queue that has already changed.
        let mut q = Queue::default();
        q.apply(
            &report(
                r#"{"playing":true,"path":"/m/a.mp3","position":0,"duration":10,
                    "queue":[{"filename":"/m/a.mp3"}],"queue_pos":7}"#,
            ),
            |_| None,
        );
        assert_eq!(q.current, Some(0));
        assert!(q.playing().is_some(), "a bad index emptied the stage");
    }

    #[test]
    fn audio_and_video_are_told_apart_by_extension() {
        assert_eq!(kind_of("/m/a.FLAC"), Kind::Audio, "case should not matter");
        assert_eq!(kind_of("/m/a.mp3"), Kind::Audio);
        assert_eq!(kind_of("/v/a.mkv"), Kind::Video);
        assert_eq!(kind_of("/v/no-extension"), Kind::Video);
    }

    #[test]
    fn streams_are_split_by_kind_and_never_left_nameless() {
        let mut q = Queue::default();
        q.apply(
            &report(
                r#"{"playing":true,"path":"/v/film.mkv","position":0,"duration":100,
                    "tracks":[{"id":1,"type":"video"},
                              {"id":1,"type":"audio","lang":"eng","selected":true},
                              {"id":2,"type":"audio","title":"Commentary","lang":"eng"},
                              {"id":1,"type":"sub","lang":"fra"},
                              {"id":2,"type":"sub"}]}"#,
            ),
            |_| None,
        );
        assert_eq!(
            q.audio.len(),
            2,
            "video stream counted as audio, or one lost"
        );
        assert_eq!(q.subtitles.len(), 2);
        assert_eq!(q.audio[0].label, "eng");
        assert_eq!(q.audio[1].label, "Commentary (eng)");
        assert!(q.audio[0].selected && !q.audio[1].selected);
        assert_eq!(
            q.subtitles[1].label, "track 2",
            "a nameless stream would be an empty row"
        );
    }

    #[test]
    fn choosing_subtitles_asks_for_them_by_id_and_can_turn_them_off() {
        let mut q = Queue {
            subtitles: vec![
                Stream {
                    id: "1".into(),
                    label: "eng".into(),
                    selected: false,
                },
                Stream {
                    id: "2".into(),
                    label: "fra".into(),
                    selected: true,
                },
            ],
            ..Queue::default()
        };
        let i = q.act(Act::Subtitles(Some("1".into()))).unwrap();
        assert_eq!(i.cap, "media.control");
        assert_eq!(i.args.str_or("action", ""), "subtitles");
        assert_eq!(i.args.str_or("track", ""), "1");
        assert!(q.subtitles[0].selected && !q.subtitles[1].selected);

        let off = q.act(Act::Subtitles(None)).unwrap();
        assert_eq!(
            off.args.str_or("track", ""),
            "no",
            "the player's word for off"
        );
        assert!(!q.subtitles.iter().any(|s| s.selected));
    }

    #[test]
    fn a_nudge_is_relative_so_it_does_not_inherit_a_stale_position() {
        // Computing an absolute target from a report that is a moment old
        // makes every nudge drift by however late that report was.
        let mut q = Queue::default();
        q.apply(&playing_one("/m/a.mp3"), |_| None);
        let i = q.act(Act::Nudge(10.0)).unwrap();
        assert_eq!(i.args.str_or("action", ""), "seek");
        assert_eq!(i.args.f64_or("seconds", 0.0), 10.0);
        assert!(i.args.get("to").is_none(), "asked for an absolute position");
    }

    #[test]
    fn enqueueing_adds_rather_than_interrupting() {
        let mut q = Queue::default();
        let i = q.act(Act::Enqueue("/m/new.mp3".into())).unwrap();
        assert_eq!(i.cap, "media.play");
        assert!(
            i.args.bool_or("queue", false),
            "this would stop what is playing"
        );
    }

    #[test]
    fn playing_a_chosen_entry_goes_to_its_place_in_the_queue() {
        let mut q = Queue::new(vec![
            Track::from_path("/m/a.mp3"),
            Track::from_path("/m/b.mp3"),
            Track::from_path("/m/c.mp3"),
        ]);
        q.selected = 2;
        let i = q.act(Act::PlaySelected).unwrap();
        assert_eq!(i.args.str_or("action", ""), "goto");
        assert_eq!(i.args.f64_or("index", -1.0), 2.0);
        assert_eq!(q.current, Some(2));
    }

    #[test]
    fn nothing_can_be_played_from_an_empty_queue() {
        let mut q = Queue::default();
        assert_eq!(q.act(Act::PlaySelected), None);
    }

    #[test]
    fn volume_and_speed_stay_within_what_the_player_accepts() {
        let mut q = Queue::default();
        q.act(Act::SetVolume(400.0));
        assert_eq!(q.volume, 100.0);
        q.act(Act::SetSpeed(99.0));
        assert_eq!(q.speed, 4.0);
        q.act(Act::SetSpeed(0.0));
        assert_eq!(q.speed, 0.25, "a stopped speed is a hang, not a setting");
    }

    #[test]
    fn selection_moves_within_the_queue_and_stops_at_its_ends() {
        let mut q = Queue::new(vec![
            Track::from_path("/m/a.mp3"),
            Track::from_path("/m/b.mp3"),
        ]);
        q.move_selection(5);
        assert_eq!(q.selected, 1);
        q.move_selection(-9);
        assert_eq!(q.selected, 0);
        let mut empty = Queue::default();
        empty.move_selection(1);
        assert_eq!(empty.selected, 0, "moved within a list that has no entries");
    }

    // --- layout and drawing ------------------------------------------------

    fn with_queue(n: usize) -> Queue {
        Queue::new(
            (0..n)
                .map(|i| Track {
                    duration: 200.0 + i as f64,
                    ..Track::from_path(&format!("/m/track-{i}.mp3"))
                })
                .collect(),
        )
    }

    #[test]
    fn the_queue_sits_beside_the_picture_and_gives_way_when_there_is_no_room() {
        let q = with_queue(4);
        let wide = Layout::compute(&q, 1100.0, 640.0);
        assert!(wide.queue.w > 0.0, "no room for a queue in 1100px");
        assert!(
            wide.stage.right() <= wide.queue.x,
            "the queue is over the picture"
        );

        let narrow = Layout::compute(&q, 520.0, 640.0);
        assert_eq!(narrow.queue.w, 0.0, "squeezed the picture to fit a list");
        assert_eq!(
            narrow.stage.w, 520.0,
            "the picture should take the whole width"
        );
    }

    #[test]
    fn stacked_parts_stay_in_order_and_inside_the_window() {
        let mut q = with_queue(3);
        q.subtitles = vec![Stream {
            id: "1".into(),
            label: "eng".into(),
            selected: true,
        }];
        let l = Layout::compute(&q, 1100.0, 640.0);
        assert!(l.stage.h > 0.0);
        assert!(
            l.streams.y >= l.stage.bottom() - 0.001,
            "stream chips overlap the picture"
        );
        assert!(l.scrub.y >= l.streams.bottom() - 0.001);
        assert!(l.transport.y >= l.scrub.bottom() - 0.001);
        assert!(
            l.transport.bottom() <= 640.0 + 0.001,
            "the controls fall off the bottom"
        );
    }

    #[test]
    fn a_file_with_no_streams_gives_their_row_back_to_the_picture() {
        let plain = Layout::compute(&with_queue(2), 1100.0, 640.0);
        let mut q = with_queue(2);
        q.audio = vec![Stream {
            id: "1".into(),
            label: "eng".into(),
            selected: true,
        }];
        let with = Layout::compute(&q, 1100.0, 640.0);
        assert!(
            plain.stage.h > with.stage.h,
            "an empty chip row still took space"
        );
        assert_eq!(plain.streams.h, 0.0);
    }

    #[test]
    fn a_short_window_does_not_produce_a_negative_stage() {
        let l = Layout::compute(&with_queue(2), 400.0, 40.0);
        assert!(l.stage.h >= 0.0, "stage height {}", l.stage.h);
        assert!(l.scrub.w >= 0.0);
    }

    #[test]
    fn a_row_scrolled_out_of_sight_cannot_be_clicked() {
        let mut q = with_queue(40);
        let l = Layout::compute(&q, 1100.0, 640.0);
        let first = l.rows[0];
        assert_eq!(l.row_at(first.x + 5.0, first.y + 5.0), Some(0));
        // Scroll the first row above the top of the list.
        q.scroll = 400.0;
        let l = Layout::compute(&q, 1100.0, 640.0);
        let hidden = l.rows[0];
        assert!(hidden.bottom() < l.queue.y, "the row is still on screen");
        assert_eq!(
            l.row_at(hidden.x + 5.0, hidden.y + 5.0),
            None,
            "clicked a row that is not there"
        );
    }

    #[test]
    fn scrolling_stops_when_the_entries_run_out() {
        let q = with_queue(40);
        let l = Layout::compute(&q, 1100.0, 640.0);
        assert!(l.max_scroll(&q) > 0.0, "forty entries fit in one screen?");
        let short = with_queue(2);
        let l2 = Layout::compute(&short, 1100.0, 640.0);
        assert_eq!(
            l2.max_scroll(&short),
            0.0,
            "a list that fits should not scroll"
        );
    }

    #[test]
    fn a_music_file_shows_something_rather_than_a_black_rectangle() {
        // mpv draws nothing at all for an mp3. Without this the surface is a
        // black void for the length of the album.
        let theme = Theme::dark();
        let mut q = Queue::default();
        q.apply(&playing_one("/home/j/Music/song.mp3"), |_| None);
        let img = Image::new(900, 600).unwrap();
        let l = Layout::compute(&q, 900.0, 600.0);
        render(&img.canvas(), &mut q, &theme, &l);
        assert!(
            img.variety(l.stage) > 4,
            "the stage is blank for an audio file"
        );
    }

    #[test]
    fn a_video_leaves_the_picture_to_the_player() {
        // The player draws its own pixels into this window. Anything painted
        // here shows through wherever the video is letterboxed.
        let theme = Theme::dark();
        let mut q = Queue::default();
        q.apply(&playing_one("/home/j/Videos/film.mkv"), |_| None);
        let img = Image::new(900, 600).unwrap();
        let l = Layout::compute(&q, 900.0, 600.0);
        render(&img.canvas(), &mut q, &theme, &l);
        let mid = img.pixel(
            (l.stage.x + l.stage.w / 2.0) as i32,
            (l.stage.y + l.stage.h / 2.0) as i32,
        );
        assert_eq!(
            mid,
            (0, 0, 0, 255),
            "something was drawn over the video: {mid:?}"
        );
    }

    #[test]
    fn the_queue_and_controls_are_actually_drawn() {
        let theme = Theme::dark();
        let mut q = with_queue(6);
        q.apply(
            &report(
                r#"{"playing":true,"path":"/m/track-1.mp3","title":"Second",
                    "position":40,"duration":200,"volume":70,"speed":1,
                    "queue":[{"filename":"/m/track-0.mp3"},{"filename":"/m/track-1.mp3","current":true}],
                    "queue_pos":1,
                    "tracks":[{"id":1,"type":"audio","lang":"eng","selected":true}]}"#,
            ),
            |_| None,
        );
        let img = Image::new(1100, 640).unwrap();
        let l = Layout::compute(&q, 1100.0, 640.0);
        render(&img.canvas(), &mut q, &theme, &l);
        assert!(img.variety(l.queue) > 5, "the queue is blank");
        assert!(img.variety(l.transport) > 5, "the controls are blank");
        assert!(img.variety(l.streams) > 3, "the stream chips are blank");
        // The played part of the scrub bar is the accent, the rest is not.
        let y = (l.scrub.y + l.scrub.h / 2.0) as i32;
        let early = img.pixel((l.scrub.x + 10.0) as i32, y);
        let late = img.pixel((l.scrub.right() - 10.0) as i32, y);
        assert_ne!(early, late, "the scrub bar shows no progress");
    }

    #[test]
    fn the_chosen_stream_is_unmistakable_in_both_themes() {
        // Which subtitle track is on is the only fact the chip row carries.
        // Two shades of the same grey reads as a row of identical chips.
        for theme in [Theme::dark(), Theme::light()] {
            let mut q = Queue::default();
            q.apply(&playing_one("/v/film.mkv"), |_| None);
            q.subtitles = vec![
                Stream {
                    id: "1".into(),
                    label: "English".into(),
                    selected: true,
                },
                Stream {
                    id: "2".into(),
                    label: "French".into(),
                    selected: false,
                },
            ];
            let img = Image::new(1100, 640).unwrap();
            let l = Layout::compute(&q, 1100.0, 640.0);
            render(&img.canvas(), &mut q, &theme, &l);

            // Walk the chip row and find the accent. Being near it is what
            // "chosen" means here, and it is a claim a grey cannot satisfy.
            let y = (l.streams.y + l.streams.h / 2.0) as i32;
            let v = theme.voice;
            let byte = |c: f64| (c * 255.0).round() as i32;
            let near = |p: (u8, u8, u8, u8)| {
                let d = (p.0 as i32 - byte(v.0)).abs()
                    + (p.1 as i32 - byte(v.1)).abs()
                    + (p.2 as i32 - byte(v.2)).abs();
                d < 40
            };
            let accent_px = (l.streams.x as i32..l.streams.right() as i32)
                .filter(|x| near(img.pixel(*x, y)))
                .count();
            assert!(
                accent_px > 20,
                "no chip is drawn in the accent: {accent_px} pixels"
            );
            // And not everything is: the unchosen chip must still be quiet.
            let row_w = (l.streams.right() - l.streams.x) as usize;
            assert!(
                accent_px < row_w / 2,
                "the whole row is accented, so nothing stands out"
            );
        }
    }

    #[test]
    fn the_volume_control_is_recognisable_as_one() {
        // A bare rule beside the clock reads as a stray line rather than a
        // control, so the speaker beside it is load-bearing.
        let theme = Theme::dark();
        let mut loud = Queue::default();
        loud.apply(&playing_one("/m/a.mp3"), |_| None);
        loud.volume = 100.0;
        let mut silent = Queue::default();
        silent.apply(&playing_one("/m/a.mp3"), |_| None);
        silent.volume = 0.0;

        let l = Layout::compute(&loud, 1100.0, 640.0);
        // The right-hand third of the controls, where the volume lives.
        let zone = Rect::new(
            l.transport.x + l.transport.w * 0.66,
            l.transport.y,
            l.transport.w * 0.34,
            l.transport.h,
        );
        let shot = |q: &mut Queue| {
            let img = Image::new(1100, 640).unwrap();
            render(&img.canvas(), q, &theme, &l);
            img
        };
        let a = shot(&mut loud);
        let b = shot(&mut silent);
        assert!(
            a.variety(zone) > 4,
            "nothing is drawn where the volume should be"
        );
        // Silence looks different from full, or the control shows nothing.
        let mut differs = 0;
        for y in zone.y as i32..zone.bottom() as i32 {
            for x in zone.x as i32..zone.right() as i32 {
                if a.pixel(x, y) != b.pixel(x, y) {
                    differs += 1;
                }
            }
        }
        assert!(
            differs > 40,
            "silence and full volume look the same: {differs} pixels differ"
        );
    }

    #[test]
    fn the_entry_playing_is_marked_differently_from_the_one_merely_chosen() {
        let theme = Theme::dark();
        let mut q = with_queue(6);
        q.current = Some(1);
        q.selected = 3;
        let img = Image::new(1100, 640).unwrap();
        let l = Layout::compute(&q, 1100.0, 640.0);
        render(&img.canvas(), &mut q, &theme, &l);
        let at = |r: Rect| img.pixel((r.x + 2.0) as i32, (r.y + r.h / 2.0) as i32);
        assert_ne!(
            at(l.rows[1]),
            at(l.rows[3]),
            "playing and selected look the same"
        );
        assert_ne!(
            at(l.rows[1]),
            at(l.rows[5]),
            "the playing entry is not marked at all"
        );
    }
}
