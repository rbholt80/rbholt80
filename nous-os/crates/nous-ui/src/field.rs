//! A folder where size means something.
//!
//! The icon grid tells one large lie: that every file matters equally. Four
//! hundred files, four hundred identical rectangles, in alphabetical order.
//! But a real folder is not four hundred equal things — it is six you are
//! working on and three hundred and ninety-four you have not thought about in
//! a year, and the grid gives you no way to tell which is which without
//! reading every name. That has been the arrangement since 1984, and it is not
//! a fact about computers. It is what you draw when the machine has no opinion
//! about your files.
//!
//! This one has an opinion, so the arrangement can carry it. Each file gets
//! area in proportion to how much it is likely to matter — touched recently,
//! large enough to be worth space, flagged by the curator — and files of a
//! kind sit together. What you were doing this week is big enough to read
//! across the room. What you have forgotten is texture along the bottom, which
//! is exactly what it is.
//!
//! Nothing here is decoration. Every visual property is a claim: area is
//! attention, position is kind, colour is what the system thinks. If a file is
//! large here and you do not care about it, the view is wrong and you should
//! be able to say so — which is a different and much better problem than a
//! grid that was never claiming anything at all.

use crate::draw::{Canvas, Picture, Rect, Rgba};
use crate::files::Entry;
use crate::theme::{Metrics, Risk, Theme};

/// How long it takes for recency to stop counting, in seconds.
///
/// Ninety days. Long enough that a project you left for a month is still
/// visible; short enough that last year's tax return is not competing with
/// what you did this morning.
const RECENCY_SPAN: f64 = 90.0 * 86400.0;

/// The shortest run of a name that is still worth showing.
///
/// Twelve characters of the widest sort. A cell narrower than this shows a
/// stub — "old-re…" — which reads as a name until you try to use it.
const LEGIBLE_STUB: &str = "mmmmmmmmmmmm";

/// The smallest a cell may be before it is not worth drawing separately.
/// Below this it is sediment, and sediment is drawn as texture.
pub const SEDIMENT: f64 = 26.0;

/// A cell's share of the folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub index: usize,
    pub rect: Rect,
    pub weight: f64,
}

impl Cell {
    /// Whether this cell is big enough to say what it is.
    ///
    /// A name is a line of text, so it needs width far more than height: a
    /// cell can be tall and narrow and still have nowhere to put one. Sixty
    /// pixels holds enough of a name to recognise it; thirty holds the line
    /// and its padding.
    pub fn readable(&self) -> bool {
        self.rect.w >= 60.0 && self.rect.h >= 30.0
    }
}

/// How much a file is likely to matter, from what can be known without asking.
///
/// Three claims, in descending confidence:
///
/// * **Recently touched things matter.** The strongest signal there is, and
///   the only one that is true of every kind of file.
/// * **Something the curator flagged matters**, because it is asking for a
///   decision. Not because it is important — because it is unresolved, and an
///   unresolved thing you cannot see is the failure mode this whole system
///   exists to fix.
/// * **Big things are more present than small ones**, weakly and
///   logarithmically. A four-gigabyte video is more of a fact about a folder
///   than a two-kilobyte note, but it is not two million times more of one.
///
/// Folders get a floor: a folder is a door, and a door too small to see is
/// worse than a door you do not need.
pub fn weight_of(e: &Entry, now: u64) -> f64 {
    if e.is_dir {
        // A door. How recently anything behind it moved is not knowable from
        // here, so every door is the same size — solidly mid-range, never
        // sediment, and never so large that it crowds out the work.
        return 2.2;
    }
    // An entry with no known time reads as old rather than as brand new: a
    // file the system has not looked at should not push aside one it has.
    let age = if e.modified == 0 {
        RECENCY_SPAN
    } else {
        now.saturating_sub(e.modified) as f64
    };
    // Falls from 1 to near 0 over the span, quickly at first: the difference
    // between today and last week matters more than between May and June.
    let recency = (1.0 - (age / RECENCY_SPAN).clamp(0.0, 1.0)).powf(1.6);

    // Logarithmic and weak. Ranges from about 0 for an empty file to about 1
    // for a few gigabytes.
    let bulk = ((e.size as f64).max(1.0).log2() / 32.0).clamp(0.0, 1.0);

    // The floor is deliberately low. A file nobody has touched in a year is
    // sediment, and a floor generous enough to keep it legible is a floor that
    // makes the whole view a grid again with extra steps.
    let base = 0.12 + recency * 3.0 + bulk * 0.35;
    match &e.mark {
        // Something waiting on a decision is pulled forward, and the more so
        // the worse the thing proposed for it.
        Some(m) => base * mark_lift(m.risk),
        None => base,
    }
}

fn mark_lift(r: Risk) -> f64 {
    match r {
        Risk::Read => 1.15,
        Risk::Write => 1.4,
        Risk::Elevated => 1.8,
        Risk::Critical => 2.2,
    }
}

/// A run of files of one kind, laid out together.
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub name: String,
    pub rect: Rect,
    pub cells: Vec<Cell>,
    pub weight: f64,
}

/// The whole arrangement.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub clusters: Vec<Cluster>,
}

impl Field {
    /// Arrange a folder.
    ///
    /// Clusters by kind, because that is what a folder is mostly made of and
    /// what people mean when they say a folder is a mess. Within a cluster,
    /// area is weight.
    pub fn arrange(entries: &[Entry], area: Rect, now: u64) -> Field {
        if entries.is_empty() || area.w <= 0.0 || area.h <= 0.0 {
            return Field {
                clusters: Vec::new(),
            };
        }
        let weights: Vec<f64> = entries.iter().map(|e| weight_of(e, now)).collect();

        // Group by kind, keeping folders first and then heaviest group first,
        // so the eye starts where the mass is.
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            let key = if e.is_dir {
                "Folders".to_string()
            } else {
                family(&e.kind())
            };
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => v.push(i),
                None => groups.push((key, vec![i])),
            }
        }
        for (_, v) in groups.iter_mut() {
            // Heaviest first within a group, so the treemap's largest cell is
            // in the corner the eye starts from.
            v.sort_by(|a, b| {
                weights[*b]
                    .partial_cmp(&weights[*a])
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| entries[*a].name.cmp(&entries[*b].name))
            });
        }
        groups.sort_by(|a, b| {
            let wa: f64 = a.1.iter().map(|i| weights[*i]).sum();
            let wb: f64 = b.1.iter().map(|i| weights[*i]).sum();
            // Folders lead whatever they weigh: they are the way out of here.
            (b.0 == "Folders")
                .cmp(&(a.0 == "Folders"))
                .then_with(|| wb.partial_cmp(&wa).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.0.cmp(&b.0))
        });

        // The clusters themselves are a treemap of their own total weights.
        let totals: Vec<f64> = groups
            .iter()
            .map(|(_, v)| v.iter().map(|i| weights[*i]).sum())
            .collect();
        let boxes = squarify(&totals, area);

        let mut clusters = Vec::new();
        for ((name, members), rect) in groups.into_iter().zip(boxes) {
            // Room for the cluster's own label along the top.
            let inner = Rect::new(
                rect.x + 2.0,
                rect.y + LABEL_H,
                (rect.w - 4.0).max(0.0),
                (rect.h - LABEL_H - 2.0).max(0.0),
            );
            let ws: Vec<f64> = members.iter().map(|i| weights[*i]).collect();
            let cell_boxes = squarify(&ws, inner);
            let cells = members
                .iter()
                .zip(cell_boxes)
                .map(|(i, r)| Cell {
                    index: *i,
                    rect: r,
                    weight: weights[*i],
                })
                .collect();
            let weight: f64 = ws.iter().sum();
            clusters.push(Cluster {
                name,
                rect,
                cells,
                weight,
            });
        }
        Field { clusters }
    }

    pub fn cells(&self) -> impl Iterator<Item = &Cell> {
        self.clusters.iter().flat_map(|c| c.cells.iter())
    }

    /// Which entry is under a point.
    pub fn hit(&self, x: f64, y: f64) -> Option<usize> {
        self.cells()
            .find(|c| c.rect.contains(x, y))
            .map(|c| c.index)
    }

    /// Where an entry ended up, for scrolling to it or ringing it.
    pub fn cell_of(&self, index: usize) -> Option<&Cell> {
        self.cells().find(|c| c.index == index)
    }
}

const LABEL_H: f64 = 19.0;

/// Group a file extension into the handful of families a person thinks in.
///
/// Not a taxonomy. The point is that pictures sit with pictures, and whether a
/// picture is a JPEG or a PNG is not something anybody arranges a folder
/// around.
pub fn family(kind: &str) -> String {
    let k = kind.to_ascii_lowercase();
    let group = match k.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "tif" | "tiff" | "bmp" | "svg" => {
            "Pictures"
        }
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" | "wmv" => "Video",
        "mp3" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "wav" | "aac" | "aiff" => "Music",
        "pdf" | "doc" | "docx" | "odt" | "rtf" | "epub" | "txt" | "md" => "Documents",
        "xls" | "xlsx" | "ods" | "csv" | "tsv" => "Spreadsheets",
        "zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar" | "iso" => "Archives",
        "deb" | "rpm" | "appimage" | "run" | "sh" | "exe" | "msi" => "Programs",
        "" | "file" | "folder" => "Other",
        _ => "Other",
    };
    group.to_string()
}

// --- treemap ---------------------------------------------------------------

/// Lay out weights as rectangles filling `area`, each in proportion to its
/// weight, kept as close to square as the shape allows.
///
/// The squarified treemap: fill a strip along the shorter side, adding cells
/// while the worst aspect ratio in the strip keeps improving, and close the
/// strip when it stops. Long thin slivers are both ugly and hard to hit, and
/// the naive alternative produces almost nothing else.
///
/// Returns one rectangle per weight, in the order given.
pub fn squarify(weights: &[f64], area: Rect) -> Vec<Rect> {
    let n = weights.len();
    if n == 0 || area.w <= 0.0 || area.h <= 0.0 {
        return vec![Rect::new(area.x, area.y, 0.0, 0.0); n];
    }
    let total: f64 = weights.iter().map(|w| w.max(0.0)).sum();
    if total <= 0.0 {
        // Everything weightless: share the space equally rather than returning
        // nothing, since these are still files that exist.
        let each = area.w / n as f64;
        return (0..n)
            .map(|i| Rect::new(area.x + i as f64 * each, area.y, each, area.h))
            .collect();
    }

    // Work in area units, so a strip's geometry is decided by numbers that do
    // not change as the remaining space shrinks.
    let scale = (area.w * area.h) / total;
    let scaled: Vec<f64> = weights.iter().map(|w| w.max(0.0) * scale).collect();

    let mut out = vec![Rect::new(area.x, area.y, 0.0, 0.0); n];
    let mut free = area;
    let mut i = 0;
    while i < n {
        let short = free.w.min(free.h);
        if short <= 0.0 {
            break;
        }
        // Grow the strip while it gets squarer.
        let mut strip_sum = scaled[i];
        let mut end = i + 1;
        let mut best = worst_ratio(strip_sum, scaled[i], scaled[i], short);
        while end < n {
            let sum = strip_sum + scaled[end];
            let lo = scaled[i..=end].iter().cloned().fold(f64::MAX, f64::min);
            let hi = scaled[i..=end].iter().cloned().fold(0.0_f64, f64::max);
            let r = worst_ratio(sum, lo, hi, short);
            if r > best {
                break;
            }
            best = r;
            strip_sum = sum;
            end += 1;
        }

        // Lay the strip along the shorter side.
        let thick = if short > 0.0 { strip_sum / short } else { 0.0 };
        let horizontal = free.w >= free.h;
        let mut at = 0.0;
        for (k, s) in scaled[i..end].iter().enumerate() {
            let along = if strip_sum > 0.0 {
                short * (s / strip_sum)
            } else {
                0.0
            };
            out[i + k] = if horizontal {
                Rect::new(free.x, free.y + at, thick.min(free.w), along)
            } else {
                Rect::new(free.x + at, free.y, along, thick.min(free.h))
            };
            at += along;
        }
        // What is left after the strip.
        free = if horizontal {
            Rect::new(free.x + thick, free.y, (free.w - thick).max(0.0), free.h)
        } else {
            Rect::new(free.x, free.y + thick, free.w, (free.h - thick).max(0.0))
        };
        i = end;
    }
    out
}

/// The worst aspect ratio a strip would have. Lower is squarer; 1 is square.
fn worst_ratio(sum: f64, lo: f64, hi: f64, short: f64) -> f64 {
    if sum <= 0.0 || short <= 0.0 || lo <= 0.0 {
        return f64::MAX;
    }
    let s2 = short * short;
    let sum2 = sum * sum;
    (s2 * hi / sum2).max(sum2 / (s2 * lo))
}

// --- drawing ---------------------------------------------------------------

/// The colour a family is drawn in.
///
/// One hue per family, at low saturation, mixed into the surface rather than
/// laid on top of it — so a folder reads as one material with regions, not as
/// a bag of coloured stickers. The hues are far enough apart to tell at a
/// glance and close enough in weight that none of them shouts.
pub fn family_hue(name: &str) -> Rgba {
    match name {
        "Folders" => Rgba::rgb(120, 140, 190),
        "Pictures" => Rgba::rgb(196, 150, 96),
        "Video" => Rgba::rgb(150, 116, 178),
        "Music" => Rgba::rgb(104, 168, 148),
        "Documents" => Rgba::rgb(126, 150, 176),
        "Spreadsheets" => Rgba::rgb(112, 168, 118),
        "Archives" => Rgba::rgb(160, 140, 112),
        "Programs" => Rgba::rgb(186, 128, 116),
        _ => Rgba::rgb(132, 136, 146),
    }
}

/// Loaded previews, kept between frames.
///
/// A `None` is a picture that failed to load, remembered so it is not retried
/// sixty times a second.
#[derive(Default)]
pub struct Pictures {
    loaded: std::collections::HashMap<String, Option<Picture>>,
}

impl Pictures {
    pub fn get(&mut self, path: &str) -> Option<&Picture> {
        self.loaded
            .entry(path.to_string())
            .or_insert_with(|| Picture::load(path).ok())
            .as_ref()
    }
}

pub fn render(
    c: &Canvas,
    field: &Field,
    entries: &[Entry],
    chosen: &[usize],
    theme: &Theme,
    area: Rect,
    pictures: &mut Pictures,
) {
    c.fill_rect(area, theme.backdrop_opaque);
    let small = theme.small_font();
    let body = theme.body_font();

    for cluster in &field.clusters {
        if cluster.rect.w < 2.0 || cluster.rect.h < 2.0 {
            continue;
        }
        let hue = family_hue(&cluster.name);
        // The cluster's own ground: the family's hue, very faint. What makes
        // regions readable without drawing a border round everything.
        c.fill_rounded(
            cluster.rect.inset(1.0),
            Metrics::RADIUS_SMALL / 2.0,
            hue.with_alpha(0.07),
        );
        if cluster.rect.h > LABEL_H + 8.0 && cluster.rect.w > 56.0 {
            c.text(
                &cluster.name,
                cluster.rect.x + 8.0,
                cluster.rect.y + 3.0,
                &small,
                hue.mix(theme.text, 0.45),
                Some(cluster.rect.w - 16.0),
            );
        }

        for cell in &cluster.cells {
            let Some(e) = entries.get(cell.index) else {
                continue;
            };
            let r = cell.rect.inset(1.5);
            if r.w <= 0.5 || r.h <= 0.5 {
                continue;
            }
            let on = chosen.contains(&cell.index);
            // Sediment: too small for a name, so it is drawn as what it is —
            // the texture of a folder's forgotten remainder.
            if r.w < SEDIMENT || r.h < SEDIMENT {
                c.fill_rect(r, hue.with_alpha(0.22));
                if on {
                    c.stroke_rounded(r, 1.5, 1.5, theme.voice);
                }
                continue;
            }

            let ground = match &e.mark {
                // A file the curator has an opinion about takes that opinion's
                // colour, so the thing waiting on a decision is the thing that
                // catches the eye.
                Some(m) => theme.risk(m.risk).with_alpha(0.30),
                None => hue.with_alpha(0.20),
            };
            c.fill_rounded(r, Metrics::RADIUS_SMALL / 2.0, ground);

            // A picture, where there is one and where there is room to see it.
            // Cropped to fill, so a row of photographs lines up rather than
            // each sitting in its own letterbox.
            let mut has_picture = false;
            if cell.readable() {
                if let Some(path) = e.thumb.clone() {
                    if let Some(pic) = pictures.get(&path) {
                        c.picture_rounded(pic, r, Metrics::RADIUS_SMALL / 2.0);
                        has_picture = true;
                    }
                }
            }
            // Over a photograph the caption needs its own ground, or a name
            // lands on whatever the picture happens to be there — which is
            // sometimes white, and then there is no name.
            if has_picture {
                let band = Rect::new(r.x, r.y, r.w, 30.0_f64.min(r.h));
                c.fill_rect(band, theme.backdrop_opaque.with_alpha(0.62));
                if e.mark.is_some() {
                    let under = Rect::new(r.x, r.y + band.h, r.w, 22.0_f64.min(r.h - band.h));
                    c.fill_rect(under, theme.backdrop_opaque.with_alpha(0.62));
                }
            }

            // Whether a name is worth drawing is measured, not guessed — but
            // measured against how much of it survives, not against a fraction
            // of its length. A fraction lets a long name through at "old-re…",
            // which looks like a name and is not one; six of those side by
            // side are worse than six blocks, since blocks do not invite you
            // to read them.
            //
            // The yardstick is a run of characters wide enough to tell two
            // files apart. Below that, nothing is written.
            let room = r.w - 16.0;
            let (full, _) = c.measure(&e.name, &body, None);
            let (least, _) = c.measure(LEGIBLE_STUB, &body, None);
            let worth_naming = cell.readable() && room >= full.min(least);
            if worth_naming {
                c.clip_rect(r);
                c.text(&e.name, r.x + 8.0, r.y + 6.0, &body, theme.text, Some(room));
                if let Some(m) = &e.mark {
                    c.text(
                        &m.note,
                        r.x + 8.0,
                        r.y + 24.0,
                        &small,
                        theme.risk(m.risk),
                        Some(room),
                    );
                } else if r.h > 52.0 {
                    c.text(
                        &e.kind(),
                        r.x + 8.0,
                        r.bottom() - 18.0,
                        &small,
                        theme.text_faint,
                        Some(room),
                    );
                }
                c.restore();
            } else {
                // Unnamed, so the only thing left to say is what family it
                // belongs to, which the colour already says. A flagged file
                // keeps its own colour even here: it is the one thing that
                // must not disappear into the texture.
                let tint = match &e.mark {
                    Some(m) => theme.risk(m.risk).with_alpha(0.34),
                    None => hue.with_alpha(0.26),
                };
                c.fill_rounded(r, Metrics::RADIUS_SMALL / 2.0, tint);
            }
            if on {
                c.stroke_rounded(r, Metrics::RADIUS_SMALL / 2.0, 1.75, theme.voice);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::Image;
    use crate::files::Mark;

    /// Touched a day ago unless a test says otherwise, so weights are decided
    /// by what each test is actually varying.
    fn entry(name: &str, size: u64) -> Entry {
        Entry {
            name: name.to_string(),
            path: format!("/nonexistent/{name}"),
            is_dir: false,
            size,
            modified: NOW - 86400,
            thumb: None,
            mark: None,
        }
    }

    fn folder(name: &str) -> Entry {
        Entry {
            is_dir: true,
            ..entry(name, 0)
        }
    }

    /// A fixed clock, so the weights a test computes do not drift with the
    /// day it is run on.
    const NOW: u64 = 1_700_000_000;

    const AREA: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 900.0,
        h: 600.0,
    };

    fn total_area(rects: &[Rect]) -> f64 {
        rects.iter().map(|r| r.w * r.h).sum()
    }

    // --- the treemap itself -----------------------------------------------

    #[test]
    fn every_cell_gets_area_in_proportion_to_its_weight() {
        // The whole claim of the view. If this is not true it is decoration.
        let w = vec![8.0, 4.0, 2.0, 1.0];
        let r = squarify(&w, AREA);
        let total: f64 = w.iter().sum();
        for (i, rect) in r.iter().enumerate() {
            let want = AREA.w * AREA.h * (w[i] / total);
            let got = rect.w * rect.h;
            assert!(
                (got - want).abs() / want < 0.02,
                "cell {i} wanted {want:.0} and got {got:.0}"
            );
        }
    }

    #[test]
    fn the_cells_fill_the_space_and_do_not_overlap() {
        let w: Vec<f64> = (1..=17).map(|i| i as f64).collect();
        let r = squarify(&w, AREA);
        let filled = total_area(&r);
        let want = AREA.w * AREA.h;
        assert!(
            (filled - want).abs() / want < 0.02,
            "filled {filled:.0} of {want:.0}"
        );
        for (i, a) in r.iter().enumerate() {
            assert!(
                a.x >= AREA.x - 0.01
                    && a.y >= AREA.y - 0.01
                    && a.right() <= AREA.right() + 0.01
                    && a.bottom() <= AREA.bottom() + 0.01,
                "cell {i} escapes the area: {a:?}"
            );
            for (j, b) in r.iter().enumerate().skip(i + 1) {
                let overlap = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0)
                    * (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
                assert!(overlap < 0.5, "cells {i} and {j} overlap by {overlap:.1}");
            }
        }
    }

    #[test]
    fn the_cells_are_not_slivers() {
        // Long thin cells are ugly and, more to the point, hard to hit. The
        // naive treemap produces almost nothing else, which is why this is
        // the squarified one.
        let w: Vec<f64> = (1..=24).map(|i| i as f64).collect();
        let r = squarify(&w, AREA);
        let mut worst: f64 = 1.0;
        for rect in &r {
            if rect.w < 1.0 || rect.h < 1.0 {
                continue;
            }
            worst = worst.max((rect.w / rect.h).max(rect.h / rect.w));
        }
        assert!(worst < 7.0, "worst aspect ratio is {worst:.1}");
    }

    #[test]
    fn one_cell_takes_the_whole_space() {
        let r = squarify(&[1.0], AREA);
        assert_eq!(r.len(), 1);
        assert!((r[0].w * r[0].h - AREA.w * AREA.h).abs() < 1.0);
    }

    #[test]
    fn nothing_to_lay_out_lays_nothing_out() {
        assert!(squarify(&[], AREA).is_empty());
        // A zero-sized area still returns one rectangle per weight, so callers
        // that zip against their input do not silently lose entries.
        assert_eq!(
            squarify(&[1.0, 2.0], Rect::new(0.0, 0.0, 0.0, 0.0)).len(),
            2
        );
    }

    #[test]
    fn weightless_files_still_get_somewhere_to_be() {
        // They exist. A view that drops them is lying about the folder.
        let r = squarify(&[0.0, 0.0, 0.0], AREA);
        assert_eq!(r.len(), 3);
        assert!(r.iter().all(|c| c.w > 0.0 && c.h > 0.0), "{r:?}");
    }

    // --- what the weights claim -------------------------------------------

    #[test]
    fn something_touched_today_outweighs_the_same_file_from_last_year() {
        // The strongest claim the view makes, and the only one true of every
        // kind of file.
        let e = entry("notes.txt", 4096);
        // weight_of reads mtime off the disk; these paths do not exist, so
        // both read as the epoch. The recency term is exercised directly.
        let fresh = 0.35 + 1.0_f64.powf(1.6) * 2.2;
        let stale = 0.35;
        assert!(
            fresh > stale * 3.0,
            "recency barely counts: {fresh} vs {stale}"
        );
        // And a missing file does not panic or produce a strange number.
        let w = weight_of(&e, NOW);
        assert!(w.is_finite() && w > 0.0, "{w}");
    }

    #[test]
    fn this_weeks_work_is_bigger_than_last_years_and_visibly_so() {
        // The whole claim of the view, end to end: not that the weights
        // differ, but that the arrangement makes the difference legible. Six
        // things you are working on among four hundred you are not is the
        // shape of every real folder, and the grid's answer to it was four
        // hundred identical rectangles.
        let mut entries = vec![Entry {
            modified: NOW - 3600,
            ..entry("report-2026.pdf", 2_400_000)
        }];
        // Three hundred, because that is what a real Downloads folder looks
        // like and because sediment only exists once a folder is genuinely
        // full — forty files in this much room are all legible whatever they
        // weigh, which is correct and tests nothing.
        for i in 0..300 {
            entries.push(Entry {
                modified: NOW - (300 + i) * 86400,
                ..entry(&format!("old-receipt-{i:03}.pdf"), 40_000)
            });
        }
        let f = Field::arrange(&entries, AREA, NOW);
        let fresh = f.cell_of(0).expect("this week's work is somewhere");
        let fresh_area = fresh.rect.w * fresh.rect.h;
        let stale_area: f64 = (1..=300)
            .filter_map(|i| f.cell_of(i))
            .map(|c| c.rect.w * c.rect.h)
            .sum();

        // One recent file against three hundred old ones takes a real share
        // of the folder rather than a three-hundredth of it.
        assert!(
            fresh_area > stale_area / 30.0,
            "this week's work got {fresh_area:.0} against {stale_area:.0} of sediment"
        );
        // And it is big enough to read, which is what "visibly" means.
        assert!(
            fresh.readable(),
            "the one thing that matters is too small to name: {:?}",
            fresh.rect
        );
        // While most of the tail is not — which is honest, not a failure.
        let sediment = (1..=300)
            .filter_map(|i| f.cell_of(i))
            .filter(|c| !c.readable())
            .count();
        assert!(
            sediment > 200,
            "only {sediment} of three hundred stale files read as sediment"
        );
    }

    #[test]
    fn a_file_waiting_on_a_decision_is_pulled_forward() {
        // Not because it is important — because it is unresolved, and an
        // unresolved thing you cannot see is the failure this system exists
        // to fix.
        let plain = entry("a.txt", 1000);
        let mut flagged = entry("b.txt", 1000);
        flagged.mark = Some(Mark {
            risk: Risk::Elevated,
            note: "duplicate".into(),
        });
        assert!(
            weight_of(&flagged, NOW) > weight_of(&plain, NOW) * 1.5,
            "a flagged file is no more visible than an ignored one"
        );
        // And the worse the proposal, the more it is pulled.
        let mut worse = flagged.clone();
        worse.mark = Some(Mark {
            risk: Risk::Critical,
            note: "delete".into(),
        });
        assert!(weight_of(&worse, NOW) > weight_of(&flagged, NOW));
    }

    #[test]
    fn size_counts_weakly_rather_than_swamping_everything() {
        // A four-gigabyte video is more of a fact about a folder than a
        // two-kilobyte note, but it is not two million times more of one.
        let small = weight_of(&entry("a.txt", 2_000), NOW);
        let huge = weight_of(&entry("b.mkv", 4_000_000_000), NOW);
        assert!(huge > small, "size counts for nothing");
        assert!(
            huge < small * 3.0,
            "size swamps everything: {small} vs {huge}"
        );
    }

    #[test]
    fn a_folder_is_never_too_small_to_see() {
        // A folder is a door. A door too small to find is worse than a door
        // you did not need.
        let d = weight_of(&folder("Invoices"), NOW);
        let stale = Entry {
            modified: NOW - 400 * 86400,
            ..entry("old.txt", 10)
        };
        let f = weight_of(&stale, NOW);
        assert!(
            d > f * 2.0,
            "a folder weighs {d} against a stale file's {f}"
        );
        // It need not outweigh today's work — a document you are writing may
        // well deserve more room than a folder you have not opened. What it
        // must never be is sediment.
        let f = Field::arrange(&[folder("Invoices"), entry("today.txt", 10)], AREA, NOW);
        assert!(
            f.cell_of(0).is_some_and(|c| c.readable()),
            "a folder was drawn too small to name"
        );
    }

    // --- the arrangement ---------------------------------------------------

    #[test]
    fn files_of_a_kind_sit_together() {
        // What people mean when they say a folder is a mess.
        let entries = vec![
            entry("a.jpg", 1000),
            entry("b.pdf", 1000),
            entry("c.jpg", 1000),
            entry("d.pdf", 1000),
            entry("e.mp3", 1000),
        ];
        let f = Field::arrange(&entries, AREA, NOW);
        let names: Vec<&str> = f.clusters.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"Pictures"), "{names:?}");
        assert!(names.contains(&"Documents"), "{names:?}");
        assert!(names.contains(&"Music"), "{names:?}");
        // Every file landed in exactly one cluster.
        let mut seen: Vec<usize> = f.cells().map(|c| c.index).collect();
        seen.sort();
        assert_eq!(seen, vec![0, 1, 2, 3, 4], "a file was lost or duplicated");
        // And the pictures are in the picture cluster.
        let pics = f.clusters.iter().find(|c| c.name == "Pictures").unwrap();
        let mut idx: Vec<usize> = pics.cells.iter().map(|c| c.index).collect();
        idx.sort();
        assert_eq!(idx, vec![0, 2]);
    }

    #[test]
    fn folders_lead_whatever_they_weigh() {
        // They are the way out of here.
        let mut entries = vec![entry("huge.mkv", 8_000_000_000)];
        entries.push(folder("Somewhere"));
        let f = Field::arrange(&entries, AREA, NOW);
        assert_eq!(f.clusters[0].name, "Folders", "the way out was buried");
    }

    #[test]
    fn a_cluster_stays_inside_its_own_box() {
        let entries: Vec<Entry> = (0..24)
            .map(|i| {
                entry(
                    &format!("f{i}.{}", ["jpg", "pdf", "mp3", "zip"][i % 4]),
                    1000 * (i as u64 + 1),
                )
            })
            .collect();
        let f = Field::arrange(&entries, AREA, NOW);
        for cl in &f.clusters {
            for cell in &cl.cells {
                assert!(
                    cell.rect.x >= cl.rect.x - 0.01
                        && cell.rect.right() <= cl.rect.right() + 0.01
                        && cell.rect.bottom() <= cl.rect.bottom() + 0.01,
                    "{} spills out of its cluster: {:?} vs {:?}",
                    entries[cell.index].name,
                    cell.rect,
                    cl.rect
                );
            }
        }
    }

    #[test]
    fn clicking_lands_on_what_was_clicked() {
        let entries: Vec<Entry> = (0..12).map(|i| entry(&format!("f{i}.jpg"), 1000)).collect();
        let f = Field::arrange(&entries, AREA, NOW);
        for cell in f.cells() {
            if cell.rect.w < 4.0 || cell.rect.h < 4.0 {
                continue;
            }
            let (x, y) = (
                cell.rect.x + cell.rect.w / 2.0,
                cell.rect.y + cell.rect.h / 2.0,
            );
            assert_eq!(f.hit(x, y), Some(cell.index), "hit the wrong cell");
        }
        assert_eq!(f.hit(-5.0, -5.0), None, "hit something outside the field");
    }

    #[test]
    fn an_empty_folder_arranges_to_nothing_rather_than_panicking() {
        let f = Field::arrange(&[], AREA, 0);
        assert!(f.clusters.is_empty());
        assert_eq!(f.hit(10.0, 10.0), None);
        // As does a folder with nowhere to draw it.
        let f = Field::arrange(&[entry("a.txt", 1)], Rect::new(0.0, 0.0, 0.0, 0.0), 0);
        assert!(f.clusters.is_empty());
    }

    #[test]
    fn the_same_folder_arranges_the_same_way_twice() {
        // A view that reshuffles between frames is unusable however pretty.
        let entries: Vec<Entry> = (0..15)
            .map(|i| entry(&format!("f{i}.{}", ["jpg", "pdf"][i % 2]), 1000))
            .collect();
        let a = Field::arrange(&entries, AREA, NOW);
        let b = Field::arrange(&entries, AREA, NOW);
        assert_eq!(a, b);
    }

    #[test]
    fn extensions_group_into_families_people_think_in() {
        assert_eq!(family("JPG"), "Pictures");
        assert_eq!(family("png"), "Pictures");
        assert_eq!(family("mkv"), "Video");
        assert_eq!(family("FLAC"), "Music");
        assert_eq!(family("xlsx"), "Spreadsheets");
        assert_eq!(family("wat"), "Other");
    }

    // --- drawing -----------------------------------------------------------

    #[test]
    fn the_field_draws_and_families_are_told_apart() {
        for theme in [Theme::dark(), Theme::light()] {
            let entries = vec![
                entry("holiday.jpg", 3_000_000),
                entry("report.pdf", 200_000),
                entry("song.flac", 40_000_000),
            ];
            let f = Field::arrange(&entries, AREA, NOW);
            let img = Image::new(900, 600).unwrap();
            render(
                &img.canvas(),
                &f,
                &entries,
                &[0],
                &theme,
                AREA,
                &mut Pictures::default(),
            );
            assert!(img.variety(AREA) > 6, "the field is blank");

            // Two different families do not draw the same colour, or the
            // arrangement carries no information.
            let sample = |name: &str| {
                let cl = f.clusters.iter().find(|c| c.name == name).unwrap();
                let cell = &cl.cells[0];
                img.pixel(
                    (cell.rect.x + cell.rect.w / 2.0) as i32,
                    (cell.rect.y + cell.rect.h / 2.0) as i32,
                )
            };
            assert_ne!(
                sample("Pictures"),
                sample("Music"),
                "pictures and music are drawn identically"
            );
        }
    }

    #[test]
    fn a_flagged_file_takes_the_colour_of_what_is_proposed_for_it() {
        // The thing waiting on a decision should be the thing that catches
        // the eye, not one more tile in a family's wash.
        let theme = Theme::dark();
        let mut entries = vec![entry("a.jpg", 1_000_000), entry("b.jpg", 1_000_000)];
        entries[1].mark = Some(Mark {
            risk: Risk::Critical,
            note: "1.8 GB, never opened".into(),
        });
        let f = Field::arrange(&entries, AREA, NOW);
        let img = Image::new(900, 600).unwrap();
        render(
            &img.canvas(),
            &f,
            &entries,
            &[],
            &theme,
            AREA,
            &mut Pictures::default(),
        );
        let at = |i: usize| {
            let c = f.cell_of(i).unwrap();
            img.pixel(
                (c.rect.x + c.rect.w / 2.0) as i32,
                (c.rect.y + c.rect.h * 0.8) as i32,
            )
        };
        assert_ne!(at(0), at(1), "a flagged file looks like an ignored one");
    }

    #[test]
    fn a_cell_too_narrow_for_a_name_shows_none_rather_than_a_stub() {
        // Fourteen cells reading "old-…" side by side are worse than fourteen
        // blocks. The blocks at least do not invite you to read them.
        let theme = Theme::dark();
        let mut entries = vec![Entry {
            modified: NOW - 3600,
            ..entry("today.pdf", 4_000_000)
        }];
        for i in 0..60 {
            entries.push(Entry {
                modified: NOW - (400 + i) * 86400,
                ..entry(&format!("old-receipt-{i:02}-final-version.pdf"), 40_000)
            });
        }
        let f = Field::arrange(&entries, AREA, NOW);
        let img = Image::new(900, 600).unwrap();
        render(
            &img.canvas(),
            &f,
            &entries,
            &[],
            &theme,
            AREA,
            &mut Pictures::default(),
        );

        // The recent one is named.
        let fresh = f.cell_of(0).unwrap();
        assert!(fresh.readable(), "{:?}", fresh.rect);
        assert!(
            img.variety(fresh.rect.inset(4.0)) > 3,
            "today's work has no name on it"
        );

        // A cell with no room for a name is a block: one flat colour, no
        // glyphs in it. "No room" is measured against the name it would have
        // to hold, which is what the drawing decides on too.
        let body = theme.body_font();
        let probe = Image::new(1, 1).unwrap();
        let unnamed = (1..=60)
            .filter_map(|i| f.cell_of(i).map(|c| (i, c)))
            .find(|(i, c)| {
                let (full, _) = probe.canvas().measure(&entries[*i].name, &body, None);
                c.rect.w > 8.0
                    && c.rect.h > 8.0
                    && (c.rect.w - 16.0)
                        < full.min(probe.canvas().measure(LEGIBLE_STUB, &body, None).0)
            })
            .map(|(_, c)| c)
            .expect("sixty long-named stale files should leave one with no room");
        // Sampled along the line the name would sit on, clear of the cell's
        // own rounded corners — whose antialiasing counts as colour variety
        // whether or not anything was written inside them.
        let r = unnamed.rect;
        let y = (r.y + 12.0) as i32;
        let band: Vec<_> = ((r.x + 6.0) as i32..(r.right() - 6.0) as i32)
            .map(|x| img.pixel(x, y))
            .collect();
        assert!(
            band.iter().all(|p| *p == band[0]),
            "a cell with no room for its name drew one anyway: {r:?}"
        );

        // And the check means something: the named cell's own line does vary.
        let fr = fresh.rect;
        let fy = (fr.y + 12.0) as i32;
        let fband: Vec<_> = ((fr.x + 6.0) as i32..(fr.right() - 6.0) as i32)
            .map(|x| img.pixel(x, fy))
            .collect();
        assert!(
            fband.iter().any(|p| *p != fband[0]),
            "nothing was written on the named cell either, so this proves nothing"
        );
    }

    #[test]
    fn a_flagged_file_stays_visible_even_when_it_is_too_small_to_name() {
        // It is the one thing that must not disappear into the texture.
        let theme = Theme::dark();
        let mut entries: Vec<Entry> = (0..80)
            .map(|i| Entry {
                modified: NOW - (400 + i) * 86400,
                ..entry(&format!("f{i:02}.pdf"), 40_000)
            })
            .collect();
        entries[40].mark = Some(Mark {
            risk: Risk::Critical,
            note: "never opened".into(),
        });
        let f = Field::arrange(&entries, AREA, NOW);
        let img = Image::new(900, 600).unwrap();
        render(
            &img.canvas(),
            &f,
            &entries,
            &[],
            &theme,
            AREA,
            &mut Pictures::default(),
        );

        let at = |i: usize| {
            let c = f.cell_of(i).unwrap();
            img.pixel(
                (c.rect.x + c.rect.w / 2.0) as i32,
                (c.rect.y + c.rect.h / 2.0) as i32,
            )
        };
        assert_ne!(
            at(40),
            at(39),
            "the flagged file sank into the texture with everything else"
        );
    }

    #[test]
    fn a_folder_of_five_thousand_files_arranges_fast_enough_to_type_through() {
        // The field is recomputed every frame. If that is slow, the whole
        // interface is slow, and it is slowest on the folders most in need of
        // it. Five thousand is a real Downloads folder that has been left
        // alone for a few years.
        let entries: Vec<Entry> = (0..5000)
            .map(|i| Entry {
                modified: NOW - (i as u64 % 900) * 86400,
                ..entry(
                    &format!(
                        "file-{i:05}.{}",
                        ["jpg", "pdf", "flac", "mp4", "zip"][i % 5]
                    ),
                    (i as u64 % 400) * 100_000 + 1000,
                )
            })
            .collect();
        let began = std::time::Instant::now();
        let f = Field::arrange(&entries, AREA, NOW);
        let took = began.elapsed();
        assert_eq!(f.cells().count(), 5000, "files were lost");
        assert!(
            took < std::time::Duration::from_millis(60),
            "arranging five thousand files took {took:?}, which is felt while typing"
        );
    }

    #[test]
    fn a_file_with_a_preview_shows_it_and_its_name_stays_readable() {
        // A photograph behind a caption is the whole point, and a caption on
        // a photograph is unreadable without a ground of its own — sometimes
        // the picture is white just there, and then there is no name.
        let dir = std::env::temp_dir().join(format!("nous-field-pic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("thumb.png");
        let src = Image::new(120, 120).unwrap();
        let sc = src.canvas();
        // Deliberately pale, so a caption drawn straight onto it would vanish.
        sc.fill_rect(Rect::new(0.0, 0.0, 120.0, 120.0), Rgba::rgb(248, 246, 240));
        src.write_png(png.to_str().unwrap()).unwrap();

        let theme = Theme::dark();
        let plain = vec![entry("holiday.jpg", 4_000_000)];
        let mut shown = plain.clone();
        shown[0].thumb = Some(png.to_string_lossy().to_string());

        let f = Field::arrange(&plain, AREA, NOW);
        let without = Image::new(900, 600).unwrap();
        render(
            &without.canvas(),
            &f,
            &plain,
            &[],
            &theme,
            AREA,
            &mut Pictures::default(),
        );
        let with = Image::new(900, 600).unwrap();
        render(
            &with.canvas(),
            &f,
            &shown,
            &[],
            &theme,
            AREA,
            &mut Pictures::default(),
        );

        let cell = f.cell_of(0).unwrap().rect;
        // The picture is there: the middle of the cell, well below the
        // caption, is not what it was.
        let mid = (
            (cell.x + cell.w / 2.0) as i32,
            (cell.y + cell.h * 0.7) as i32,
        );
        assert_ne!(
            without.pixel(mid.0, mid.1),
            with.pixel(mid.0, mid.1),
            "the preview was not drawn"
        );

        // And the name is still legible: the strip it sits on is darker than
        // the pale picture below it.
        let band = with.pixel((cell.x + cell.w - 12.0) as i32, (cell.y + 14.0) as i32);
        let body = with.pixel(
            (cell.x + cell.w - 12.0) as i32,
            (cell.y + cell.h * 0.7) as i32,
        );
        let lum = |p: (u8, u8, u8, u8)| p.0 as i32 + p.1 as i32 + p.2 as i32;
        assert!(
            lum(band) < lum(body) - 60,
            "the caption has no ground under it: band {band:?} against picture {body:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cell_too_small_for_a_name_gets_no_preview_either() {
        // A cell that cannot show a name cannot show a photograph, and
        // decoding one to draw it at forty pixels is work for nothing.
        let entries: Vec<Entry> = (0..300)
            .map(|i| Entry {
                modified: NOW - (300 + i as u64) * 86400,
                ..entry(&format!("f{i:03}.jpg"), 40_000)
            })
            .collect();
        let f = Field::arrange(&entries, AREA, NOW);
        let tiny = f.cells().filter(|c| !c.readable()).count();
        assert!(tiny > 200, "only {tiny} of three hundred are too small");
    }

    #[test]
    fn the_selection_is_as_clear_in_one_theme_as_the_other() {
        // A ring that reads as bright on a dark ground can be a hairline on a
        // pale one, and "which files am I about to delete" is not a question
        // to answer by squinting.
        let entries: Vec<Entry> = (0..6)
            .map(|i| entry(&format!("f{i}.jpg"), 1_000_000))
            .collect();
        let f = Field::arrange(&entries, AREA, NOW);
        let c = f.cell_of(0).unwrap().rect;
        let mut counts: Vec<i32> = Vec::new();
        for theme in [Theme::dark(), Theme::light()] {
            let shot = |sel: &[usize]| {
                let img = Image::new(900, 600).unwrap();
                render(
                    &img.canvas(),
                    &f,
                    &entries,
                    sel,
                    &theme,
                    AREA,
                    &mut Pictures::default(),
                );
                img
            };
            let none = shot(&[]);
            let one = shot(&[0]);
            let mut differs = 0;
            for y in c.y as i32..c.bottom() as i32 {
                for x in c.x as i32..c.right() as i32 {
                    if none.pixel(x, y) != one.pixel(x, y) {
                        differs += 1;
                    }
                }
            }
            assert!(differs > 20, "the selection is invisible: {differs} pixels");
            counts.push(differs);
        }
        let (a, b) = (counts[0] as f64, counts[1] as f64);
        assert!(
            a.min(b) / a.max(b) > 0.5,
            "the selection is far weaker in one theme than the other: {counts:?}"
        );
    }

    #[test]
    fn several_chosen_cells_are_all_marked() {
        // A multiple selection is only trustworthy if you can see all of it.
        let theme = Theme::dark();
        let entries: Vec<Entry> = (0..6)
            .map(|i| entry(&format!("f{i}.jpg"), 1_000_000))
            .collect();
        let f = Field::arrange(&entries, AREA, NOW);
        let img = Image::new(900, 600).unwrap();
        render(
            &img.canvas(),
            &f,
            &entries,
            &[1, 3],
            &theme,
            AREA,
            &mut Pictures::default(),
        );
        let plain = Image::new(900, 600).unwrap();
        render(
            &plain.canvas(),
            &f,
            &entries,
            &[],
            &theme,
            AREA,
            &mut Pictures::default(),
        );

        for i in [1usize, 3] {
            let c = f.cell_of(i).unwrap().rect;
            let mut differs = 0;
            for y in c.y as i32..c.bottom() as i32 {
                for x in c.x as i32..c.right() as i32 {
                    if img.pixel(x, y) != plain.pixel(x, y) {
                        differs += 1;
                    }
                }
            }
            assert!(differs > 20, "cell {i} is chosen and unmarked");
        }
        let c = f.cell_of(2).unwrap().rect;
        assert_eq!(
            img.pixel((c.x + c.w / 2.0) as i32, (c.y + 4.0) as i32),
            plain.pixel((c.x + c.w / 2.0) as i32, (c.y + 4.0) as i32),
            "an unchosen cell was marked"
        );
    }
}
