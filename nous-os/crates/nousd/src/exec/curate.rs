//! The curator: the part of the system that keeps your files tidy.
//!
//! Curation is split into three capabilities on purpose:
//!
//! - `curate.scan` looks and reports. It is read-only and always safe to run.
//! - `curate.propose` turns findings into concrete, reviewable steps.
//! - `curate.apply` executes a proposal, one journalled step at a time.
//!
//! Nothing is ever tidied behind your back. The scan runs on a timer, the
//! proposal is shown to you, and applying it is a decision you make. Every step
//! it takes is a `fs.move` — the curator has no capability to delete.

use super::{Effect, ExecCtx};
use nous_core::cap::Capability;
use nous_core::journal::now_secs;
use nous_core::json::{json_obj, Json};
use nous_core::Step;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

pub fn execute(cap: &Capability, step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    match cap.action.as_str() {
        "scan" => scan(step, ctx),
        "propose" => propose(step, ctx),
        "apply" => apply(step, ctx),
        other => Err(format!("curator cannot '{}'", other)),
    }
}

const DAY: u64 = 86_400;

/// A thing worth telling the user about.
#[derive(Debug, Clone)]
pub struct Finding {
    pub kind: &'static str,
    pub severity: u8,
    pub title: String,
    pub detail: String,
    pub paths: Vec<PathBuf>,
    pub bytes: u64,
}

impl Finding {
    fn to_json(&self) -> Json {
        json_obj([
            ("kind", self.kind.into()),
            ("severity", (self.severity as u64).into()),
            ("title", self.title.clone().into()),
            ("detail", self.detail.clone().into()),
            ("bytes", self.bytes.into()),
            (
                "paths",
                Json::Arr(
                    self.paths
                        .iter()
                        .map(|p| Json::Str(p.to_string_lossy().to_string()))
                        .collect(),
                ),
            ),
        ])
    }
}

fn roots(step: &Step, ctx: &ExecCtx) -> Vec<PathBuf> {
    if let Some(list) = step.args.get("roots").and_then(|v| v.as_arr()) {
        return list
            .iter()
            .filter_map(|v| v.as_str())
            .map(nous_core::config::expand_tilde)
            .collect();
    }
    let home = ctx.home.clone();
    [
        "Downloads",
        "Desktop",
        "Documents",
        "Pictures",
        "Videos",
        "Music",
    ]
    .iter()
    .map(|d| home.join(d))
    .filter(|p| p.is_dir())
    .collect()
}

/// Look for clutter. Read-only.
pub fn analyse(roots: &[PathBuf], exclude: &[String], now: u64) -> Vec<Finding> {
    let mut files = Vec::new();
    for r in roots {
        super::fsops::walk(r, 8, exclude, &mut files, 40_000);
    }

    let mut findings = Vec::new();
    findings.extend(find_duplicates(&files));
    findings.extend(find_stale_downloads(roots, now));
    findings.extend(find_screenshot_clutter(roots));
    findings.extend(find_large_files(&files));
    findings.extend(find_misfiled_media(roots));
    findings.extend(find_arrived_together(roots));
    findings.extend(find_loose_by_kind(roots));
    findings.extend(find_empty_dirs(roots));

    // Most consequential first: the user should see the 40 GB of duplicates
    // before the three empty folders.
    findings.sort_by(|a, b| b.severity.cmp(&a.severity).then(b.bytes.cmp(&a.bytes)));
    findings
}

/// Content hash of a file, cheap enough to run across a home directory.
///
/// Reads at most the head and tail plus the exact length, which distinguishes
/// files reliably in practice; exact byte comparison then confirms every
/// candidate pair before anything is proposed.
fn quick_hash(path: &Path) -> Option<(u64, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let md = std::fs::metadata(path).ok()?;
    let len = md.len();
    if len == 0 {
        return None;
    }
    let mut f = std::fs::File::open(path).ok()?;
    let window = 64 * 1024;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut buf = vec![0u8; window.min(len as usize)];
    f.read_exact(&mut buf).ok()?;
    for b in &buf {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    if len > window as u64 * 2 {
        f.seek(SeekFrom::End(-(window as i64))).ok()?;
        f.read_exact(&mut buf).ok()?;
        for b in &buf {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Some((len, hash))
}

/// Byte-for-byte comparison. A hash collision must never cost someone a file.
fn same_content(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (mut fa, mut fb) = match (std::fs::File::open(a), std::fs::File::open(b)) {
        (Ok(x), Ok(y)) => (x, y),
        _ => return false,
    };
    let (mut ba, mut bb) = ([0u8; 32 * 1024], [0u8; 32 * 1024]);
    loop {
        let na = match fa.read(&mut ba) {
            Ok(n) => n,
            Err(_) => return false,
        };
        let nb = match fb.read(&mut bb) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if na != nb {
            return false;
        }
        if na == 0 {
            return true;
        }
        if ba[..na] != bb[..nb] {
            return false;
        }
    }
}

fn find_duplicates(files: &[PathBuf]) -> Vec<Finding> {
    let mut by_key: HashMap<(u64, u64), Vec<PathBuf>> = HashMap::new();
    for f in files {
        if let Some(key) = quick_hash(f) {
            by_key.entry(key).or_default().push(f.clone());
        }
    }

    let mut out = Vec::new();
    for ((len, _), group) in by_key {
        if group.len() < 2 {
            continue;
        }
        // Confirm each candidate against the first before calling it a copy.
        let mut confirmed = vec![group[0].clone()];
        for other in &group[1..] {
            if same_content(&group[0], other) {
                confirmed.push(other.clone());
            }
        }
        if confirmed.len() < 2 {
            continue;
        }
        // Keep the oldest copy; it is most likely the original.
        confirmed.sort_by_key(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
        let wasted = len * (confirmed.len() as u64 - 1);
        out.push(Finding {
            kind: "duplicate",
            severity: if wasted > 100 * 1024 * 1024 { 4 } else { 3 },
            title: format!(
                "{} identical copies of {}",
                confirmed.len(),
                confirmed[0]
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("a file")
            ),
            detail: format!(
                "{} wasted; the oldest copy would be kept",
                human_bytes(wasted)
            ),
            paths: confirmed,
            bytes: wasted,
        });
    }
    out
}

fn find_stale_downloads(roots: &[PathBuf], now: u64) -> Vec<Finding> {
    let downloads = match roots.iter().find(|r| r.ends_with("Downloads")) {
        Some(d) => d.clone(),
        None => return Vec::new(),
    };
    let mut stale = Vec::new();
    let mut bytes = 0u64;
    if let Ok(dir) = std::fs::read_dir(&downloads) {
        for e in dir.flatten() {
            let p = e.path();
            let md = match e.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let age = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| now.saturating_sub(d.as_secs()))
                .unwrap_or(0);
            if age > 90 * DAY {
                bytes += md.len();
                stale.push(p);
            }
        }
    }
    if stale.len() < 3 {
        return Vec::new();
    }
    stale.sort();
    Vec::from([Finding {
        kind: "stale_downloads",
        severity: 2,
        title: format!("{} downloads untouched for over three months", stale.len()),
        detail: format!(
            "{} that could move to an archive folder",
            human_bytes(bytes)
        ),
        paths: stale,
        bytes,
    }])
}

fn find_screenshot_clutter(roots: &[PathBuf]) -> Vec<Finding> {
    let mut shots = Vec::new();
    let mut bytes = 0u64;
    for root in roots {
        if let Ok(dir) = std::fs::read_dir(root) {
            for e in dir.flatten() {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                let looks_like_a_screenshot = name.starts_with("screenshot")
                    || name.starts_with("screen shot")
                    || name.starts_with("scr-")
                    || (name.starts_with("image") && name.contains("png"));
                if looks_like_a_screenshot {
                    bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                    shots.push(e.path());
                }
            }
        }
    }
    if shots.len() < 5 {
        return Vec::new();
    }
    shots.sort();
    Vec::from([Finding {
        kind: "screenshots",
        severity: 2,
        title: format!("{} screenshots loose in your folders", shots.len()),
        detail: format!(
            "{} that could gather into Pictures/Screenshots",
            human_bytes(bytes)
        ),
        paths: shots,
        bytes,
    }])
}

fn find_large_files(files: &[PathBuf]) -> Vec<Finding> {
    let mut big: Vec<(u64, PathBuf)> = files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| (m.len(), p.clone())))
        .filter(|(len, _)| *len > 1024 * 1024 * 1024)
        .collect();
    if big.is_empty() {
        return Vec::new();
    }
    big.sort_by(|a, b| b.0.cmp(&a.0));
    let total: u64 = big.iter().map(|(l, _)| l).sum();
    Vec::from([Finding {
        kind: "large_files",
        // Informational: a big file is not a problem, it is just worth knowing.
        severity: 1,
        title: format!("{} files over 1 GB", big.len()),
        detail: format!(
            "{} in total; the largest is {}",
            human_bytes(total),
            human_bytes(big[0].0)
        ),
        paths: big.into_iter().take(20).map(|(_, p)| p).collect(),
        bytes: total,
    }])
}

/// Music and video sitting in Downloads instead of the library folders.
fn find_misfiled_media(roots: &[PathBuf]) -> Vec<Finding> {
    let downloads = match roots.iter().find(|r| r.ends_with("Downloads")) {
        Some(d) => d.clone(),
        None => return Vec::new(),
    };
    let mut media = Vec::new();
    let mut bytes = 0u64;
    if let Ok(dir) = std::fs::read_dir(&downloads) {
        for e in dir.flatten() {
            let p = e.path();
            let kind = super::fsops::classify(&p, p.is_dir());
            if kind == "audio" || kind == "video" {
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                media.push(p);
            }
        }
    }
    if media.is_empty() {
        return Vec::new();
    }
    media.sort();
    Vec::from([Finding {
        kind: "misfiled_media",
        severity: 3,
        title: format!("{} media files sitting in Downloads", media.len()),
        detail: format!("{} that belong in Music or Videos", human_bytes(bytes)),
        paths: media,
        bytes,
    }])
}

/// Folders worth sorting. Only the two the user actually tips things into --
/// reorganising a home directory nobody asked about would be presumptuous, and
/// a project folder's "mess" is usually its structure.
fn sortable_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .filter(|r| r.ends_with("Downloads") || r.ends_with("Documents"))
        .cloned()
        .collect()
}

/// The loose files sitting directly in `dir`, with their kind, size and age.
/// Not recursive: a file already inside a subfolder has been filed.
fn loose_files(dir: &Path) -> Vec<(PathBuf, &'static str, u64, u64)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            continue;
        }
        // A dotfile in Downloads is a browser's business, not the user's.
        if p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push((
            p.clone(),
            super::fsops::classify(&p, false),
            meta.len(),
            mtime,
        ));
    }
    out.sort();
    out
}

/// The folder a kind of file belongs in, in words a person would use. `None`
/// for kinds not worth gathering: one stray `.conf` is not a category.
fn kind_folder(kind: &str) -> Option<&'static str> {
    match kind {
        "image" => Some("Images"),
        "document" => Some("Documents"),
        "sheet" => Some("Spreadsheets"),
        "slides" => Some("Presentations"),
        "archive" => Some("Archives"),
        "package" => Some("Installers"),
        "text" => Some("Notes"),
        "code" => Some("Code"),
        "image-disk" => Some("Disk Images"),
        _ => None,
    }
}

/// How many loose files of one kind make a folder worth creating. Below this a
/// subfolder is more clutter than the files were.
const GATHER_THRESHOLD: usize = 4;

/// Loose files of the same kind piling up in Downloads or Documents.
///
/// The plainest kind of tidying and the one most people do by hand: everything
/// of a type into a folder named for that type.
fn find_loose_by_kind(roots: &[PathBuf]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for dir in sortable_roots(roots) {
        let mut by_kind: BTreeMap<&'static str, (Vec<PathBuf>, u64)> = BTreeMap::new();
        for (path, kind, size, _) in loose_files(&dir) {
            if kind_folder(kind).is_none() {
                continue;
            }
            let slot = by_kind.entry(kind).or_default();
            slot.0.push(path);
            slot.1 += size;
        }
        for (kind, (paths, bytes)) in by_kind {
            if paths.len() < GATHER_THRESHOLD {
                continue;
            }
            let folder = kind_folder(kind).unwrap_or("Sorted");
            findings.push(Finding {
                kind: "loose_by_kind",
                severity: 3,
                title: format!(
                    "{} loose {} files in {}",
                    paths.len(),
                    kind,
                    dir.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("that folder")
                ),
                detail: format!("{} that could go into {}", human_bytes(bytes), folder),
                paths,
                bytes,
            });
        }
    }
    findings
}

/// The stem a file shares with its siblings: its name with any trailing part
/// number stripped, so `invoice-part1` and `invoice-part2` agree on `invoice`.
///
/// Deliberately conservative. Only a trailing run of digits, optionally behind
/// a `part`/`disc`/`cd`/`vol` word and a separator, is removed -- `report_data`
/// keeps its whole name, because `data` is a word, not an index.
fn shared_stem(name: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let mut s = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    if s.len() == stem.len() {
        return stem.to_string();
    }
    s = s.trim_end_matches([' ', '-', '_', '.']);
    // Strip a marker word sitting in front of the index -- "manual-part1"
    // means "manual". Only when something is left afterwards: for
    // "chapter_03" the word *is* the whole name, and removing it would leave
    // nothing to group on.
    for word in ["part", "disc", "cd", "vol", "page", "chapter"] {
        if s.len() > word.len() && s[s.len() - word.len()..].eq_ignore_ascii_case(word) {
            let rest = s[..s.len() - word.len()].trim_end_matches([' ', '-', '_', '.']);
            if !rest.is_empty() {
                s = &s[..s.len() - word.len()];
                break;
            }
        }
    }
    s.trim_end_matches([' ', '-', '_', '.']).to_string()
}

/// How close together files must have arrived to count as one delivery.
const TOGETHER_WINDOW: u64 = 300;
/// How many of them it takes. Two files with a shared stem is a coincidence.
const TOGETHER_THRESHOLD: usize = 3;

/// Files that arrived as one thing and were scattered across the folder.
///
/// Two independent signals have to agree before anything is proposed: the names
/// share a stem, *and* they were written within a few minutes of each other. A
/// single signal is not enough -- `report.pdf` and `report.pdf` downloaded a
/// year apart are not a set, and forty unrelated files from one busy afternoon
/// are not either. Grouping the wrong things is worse than grouping nothing.
fn find_arrived_together(roots: &[PathBuf]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for dir in sortable_roots(roots) {
        let mut by_stem: BTreeMap<String, Vec<(PathBuf, u64, u64)>> = BTreeMap::new();
        for (path, _, size, mtime) in loose_files(&dir) {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let stem = shared_stem(name);
            // A one- or two-character stem says nothing about relatedness.
            if stem.len() < 3 {
                continue;
            }
            by_stem.entry(stem).or_default().push((path, size, mtime));
        }

        for (stem, group) in by_stem {
            if group.len() < TOGETHER_THRESHOLD {
                continue;
            }
            let times: Vec<u64> = group.iter().map(|(_, _, t)| *t).collect();
            let (lo, hi) = (
                times.iter().copied().min().unwrap_or(0),
                times.iter().copied().max().unwrap_or(0),
            );
            // Unknown timestamps read as 0 and would make any group look
            // simultaneous. A group is only "together" if it really is dated.
            if lo == 0 || hi.saturating_sub(lo) > TOGETHER_WINDOW {
                continue;
            }
            let bytes: u64 = group.iter().map(|(_, s, _)| *s).sum();
            findings.push(Finding {
                kind: "arrived_together",
                severity: 4,
                title: format!("{} files that arrived together as \"{stem}\"", group.len()),
                detail: format!(
                    "{} downloaded within a few minutes of each other",
                    human_bytes(bytes)
                ),
                paths: group.into_iter().map(|(p, _, _)| p).collect(),
                bytes,
            });
        }
    }
    findings
}

fn find_empty_dirs(roots: &[PathBuf]) -> Vec<Finding> {
    let mut empties = Vec::new();
    for root in roots {
        collect_empty(root, 4, &mut empties);
    }
    if empties.len() < 3 {
        return Vec::new();
    }
    empties.sort();
    Vec::from([Finding {
        kind: "empty_dirs",
        severity: 1,
        title: format!("{} empty folders", empties.len()),
        detail: "left behind by moves and unpacked archives".to_string(),
        paths: empties,
        bytes: 0,
    }])
}

fn collect_empty(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e.flatten().collect::<Vec<_>>(),
        Err(_) => return,
    };
    if entries.is_empty() {
        out.push(dir.to_path_buf());
        return;
    }
    for e in entries {
        if e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.') {
            collect_empty(&e.path(), depth - 1, out);
        }
    }
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, UNITS[0])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

fn scan(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let rs = roots(step, ctx);
    let findings = analyse(&rs, &ctx.cfg.list("index.exclude"), now_secs());
    let reclaimable: u64 = findings
        .iter()
        .filter(|f| f.kind == "duplicate" || f.kind == "stale_downloads")
        .map(|f| f.bytes)
        .sum();

    Ok(Effect::read_only(
        json_obj([
            (
                "findings",
                Json::Arr(findings.iter().map(|f| f.to_json()).collect()),
            ),
            ("count", findings.len().into()),
            ("reclaimable_bytes", reclaimable.into()),
            ("reclaimable", human_bytes(reclaimable).into()),
            (
                "roots",
                Json::Arr(
                    rs.iter()
                        .map(|r| Json::Str(r.to_string_lossy().to_string()))
                        .collect(),
                ),
            ),
        ]),
        format!(
            "found {} things to tidy ({} reclaimable)",
            findings.len(),
            human_bytes(reclaimable)
        ),
    ))
}

/// Which finding gets to decide a file's fate when several apply to it.
///
/// A file can easily be both "a duplicate" and "media in the wrong folder".
/// Filing it into Music *and* moving it to the duplicates tray are mutually
/// exclusive, and the first move would make the second fail. Duplicate handling
/// wins because it is the more consequential judgement: there is no point
/// tidying a file into your library that you are about to be shown as a copy.
fn kind_priority(kind: &str) -> u8 {
    match kind {
        "duplicate" => 0,
        "misfiled_media" => 1,
        "screenshots" => 2,
        // A delivery is planned before the plain by-type sweep so that its
        // files claim their destination first -- that destination is *inside*
        // the type folder, so type still organises the result.
        "arrived_together" => 3,
        "stale_downloads" => 4,
        "loose_by_kind" => 5,
        _ => 9,
    }
}

/// Turn findings into concrete steps. Every step is a move; the curator never
/// proposes a delete.
pub fn plan_steps(findings: &[Finding], home: &Path, kinds: &[String]) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut n = 0;
    // Each source path may be moved at most once, and destinations must be
    // unique too -- two files called `scan.pdf` from different folders would
    // otherwise be proposed into the same destination, and the second move
    // would refuse to clobber the first.
    let mut claimed_sources: std::collections::HashSet<PathBuf> = Default::default();
    let mut claimed_dests: std::collections::HashSet<PathBuf> = Default::default();

    let mut ordered: Vec<&Finding> = findings.iter().collect();
    ordered.sort_by_key(|f| kind_priority(f.kind));

    for f in ordered {
        if !kinds.is_empty() && !kinds.iter().any(|k| k == f.kind) {
            continue;
        }
        let targets: Vec<(PathBuf, PathBuf)> = match f.kind {
            // Keep the first (oldest) copy; the rest go to a review folder
            // rather than the trash, so nothing disappears on the user.
            "duplicate" => f.paths[1..]
                .iter()
                .map(|p| (p.clone(), home.join("Tidy/Duplicates").join(unique_name(p))))
                .collect(),
            "stale_downloads" => f
                .paths
                .iter()
                .map(|p| {
                    (
                        p.clone(),
                        home.join("Tidy/Old Downloads").join(unique_name(p)),
                    )
                })
                .collect(),
            "screenshots" => f
                .paths
                .iter()
                .map(|p| {
                    (
                        p.clone(),
                        home.join("Pictures/Screenshots").join(unique_name(p)),
                    )
                })
                .collect(),
            "misfiled_media" => f
                .paths
                .iter()
                .map(|p| {
                    let dest = if super::fsops::classify(p, false) == "audio" {
                        home.join("Music")
                    } else {
                        home.join("Videos")
                    };
                    (p.clone(), dest.join(unique_name(p)))
                })
                .collect(),
            // Into a folder named for the delivery, beside the files it came
            // from -- not off in Tidy/, because these are wanted.
            "arrived_together" => {
                let stem = f
                    .title
                    .rsplit_once('"')
                    .and_then(|(head, _)| head.rsplit_once('"').map(|(_, s)| s.to_string()))
                    .unwrap_or_else(|| "Group".to_string());
                // Every file of one type, or a mixture?
                let kinds: Vec<&str> = f
                    .paths
                    .iter()
                    .map(|p| super::fsops::classify(p, false))
                    .collect();
                let shared: Option<&'static str> = match kinds.first() {
                    Some(first) if kinds.iter().all(|k| k == first) => kind_folder(first),
                    _ => None,
                };
                f.paths
                    .iter()
                    .map(|p| {
                        let parent = p
                            .parent()
                            .map_or_else(|| home.to_path_buf(), Path::to_path_buf);
                        let dest = match shared {
                            Some(folder) => parent.join(folder).join(&stem),
                            None => parent.join(&stem),
                        };
                        (p.clone(), dest.join(unique_name(p)))
                    })
                    .collect()
            }
            // Into a folder named for the type, inside the folder they are
            // already in. Downloads/Images, Documents/Spreadsheets.
            "loose_by_kind" => f
                .paths
                .iter()
                .map(|p| {
                    let parent = p
                        .parent()
                        .map_or_else(|| home.to_path_buf(), Path::to_path_buf);
                    let folder = kind_folder(super::fsops::classify(p, false)).unwrap_or("Sorted");
                    (p.clone(), parent.join(folder).join(unique_name(p)))
                })
                .collect(),
            // Large files and empty folders are reported, never acted on:
            // the system has no basis for deciding a big file is unwanted.
            _ => Vec::new(),
        };

        for (from, mut to) in targets {
            if !claimed_sources.insert(from.clone()) {
                continue;
            }
            // Disambiguate a colliding destination rather than dropping the move.
            if claimed_dests.contains(&to) || to.exists() {
                let stem = to
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("item")
                    .to_string();
                let ext = to
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|e| format!(".{}", e))
                    .unwrap_or_default();
                let parent = to.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                let mut i = 2;
                loop {
                    let candidate = parent.join(format!("{} ({}){}", stem, i, ext));
                    if !claimed_dests.contains(&candidate) && !candidate.exists() {
                        to = candidate;
                        break;
                    }
                    i += 1;
                }
            }
            claimed_dests.insert(to.clone());
            n += 1;
            steps.push(Step::new(
                &format!("tidy{}", n),
                &format!("fs.move:{}", from.display()),
                "fs",
                &format!(
                    "move {} to {}",
                    from.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                    to.parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default()
                ),
                json_obj([
                    ("from", from.to_string_lossy().to_string().into()),
                    ("to", to.to_string_lossy().to_string().into()),
                ]),
            ));
        }
    }
    steps
}

fn unique_name(p: &Path) -> String {
    p.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("item")
        .to_string()
}

fn propose(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let rs = roots(step, ctx);
    let kinds = step.args.str_list("kinds");
    let findings = analyse(&rs, &ctx.cfg.list("index.exclude"), now_secs());
    let steps = plan_steps(&findings, &ctx.home, &kinds);
    let moved: u64 = findings
        .iter()
        .filter(|f| kinds.is_empty() || kinds.iter().any(|k| k == f.kind))
        .map(|f| f.bytes)
        .sum();

    Ok(Effect::read_only(
        json_obj([
            (
                "steps",
                Json::Arr(steps.iter().map(|s| s.to_json()).collect()),
            ),
            ("count", steps.len().into()),
            ("bytes", moved.into()),
            ("summary", human_bytes(moved).into()),
            (
                "findings",
                Json::Arr(findings.iter().map(|f| f.to_json()).collect()),
            ),
        ]),
        format!(
            "proposed {} moves affecting {}",
            steps.len(),
            human_bytes(moved)
        ),
    ))
}

/// Applying a proposal is **not** done here.
///
/// The first live run of this code moved nine files and left them unreversible,
/// because executing the moves inside the executor bypassed the broker: no
/// policy check per move, and no journal entry per move, so there was nothing
/// for undo to reverse. The curator's job ends at proposing. The broker expands
/// a proposal and runs each move through the ordinary governed path, which is
/// what makes every one of them individually undoable.
fn apply(_step: &Step, _ctx: &ExecCtx) -> Result<Effect, String> {
    Err("curate.apply is expanded by the broker, not executed here".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("nous-curate-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn finds_exact_duplicates_and_keeps_the_oldest() {
        let dir = scratch("dupes");
        let a = dir.join("original.bin");
        let b = dir.join("copy.bin");
        let c = dir.join("different.bin");
        fs::write(&a, vec![7u8; 200_000]).unwrap();
        fs::write(&b, vec![7u8; 200_000]).unwrap();
        fs::write(&c, vec![9u8; 200_000]).unwrap();

        let mut files = Vec::new();
        super::super::fsops::walk(&dir, 4, &[], &mut files, 100);
        let dupes: Vec<_> = find_duplicates(&files)
            .into_iter()
            .filter(|f| f.kind == "duplicate")
            .collect();

        assert_eq!(dupes.len(), 1, "the distinct file must not be grouped in");
        assert_eq!(dupes[0].paths.len(), 2);
        assert_eq!(
            dupes[0].bytes, 200_000,
            "one file's worth is wasted, not two"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_size_different_content_is_not_a_duplicate() {
        let dir = scratch("collide");
        let a = dir.join("a.bin");
        let b = dir.join("b.bin");
        let mut da = vec![1u8; 300_000];
        let mut db = vec![1u8; 300_000];
        // Differ only in the middle, which the head/tail hash cannot see: the
        // byte-for-byte confirmation is what has to catch this.
        da[150_000] = 2;
        db[150_000] = 3;
        fs::write(&a, &da).unwrap();
        fs::write(&b, &db).unwrap();

        let files = vec![a.clone(), b.clone()];
        assert!(
            find_duplicates(&files).is_empty(),
            "content differs, so these are not copies"
        );
        assert!(!same_content(&a, &b));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn proposals_only_ever_move_never_delete() {
        let home = scratch("proposal");
        fs::create_dir_all(home.join("Downloads")).unwrap();
        let f = Finding {
            kind: "misfiled_media",
            severity: 3,
            title: "t".into(),
            detail: "d".into(),
            paths: vec![
                home.join("Downloads/song.mp3"),
                home.join("Downloads/clip.mp4"),
            ],
            bytes: 100,
        };
        let steps = plan_steps(&[f], &home, &[]);
        assert_eq!(steps.len(), 2);
        assert!(steps.iter().all(|s| s.capability.starts_with("fs.move:")));
        assert!(steps[0].args.str_or("to", "").ends_with("Music/song.mp3"));
        assert!(steps[1].args.str_or("to", "").ends_with("Videos/clip.mp4"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn duplicate_proposals_spare_the_first_copy() {
        let home = scratch("spare");
        let f = Finding {
            kind: "duplicate",
            severity: 4,
            title: "t".into(),
            detail: "d".into(),
            paths: vec![
                home.join("keep.bin"),
                home.join("dupe1.bin"),
                home.join("dupe2.bin"),
            ],
            bytes: 20,
        };
        let steps = plan_steps(&[f], &home, &[]);
        assert_eq!(steps.len(), 2, "only the extra copies move");
        assert!(steps
            .iter()
            .all(|s| !s.args.str_or("from", "").ends_with("keep.bin")));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_file_is_never_proposed_for_two_destinations() {
        // The case that showed up the first time this ran against a real home
        // directory: a duplicate mp3 sitting in Downloads is both "a duplicate"
        // and "misfiled media", and was proposed for both.
        let home = scratch("conflict");
        let dupe = home.join("Downloads/album-track-copy.mp3");
        let misfiled = Finding {
            kind: "misfiled_media",
            severity: 3,
            title: "t".into(),
            detail: "d".into(),
            paths: vec![home.join("Downloads/album-track.mp3"), dupe.clone()],
            bytes: 10,
        };
        let duplicate = Finding {
            kind: "duplicate",
            severity: 3,
            title: "t".into(),
            detail: "d".into(),
            paths: vec![home.join("Downloads/album-track.mp3"), dupe.clone()],
            bytes: 10,
        };

        let steps = plan_steps(&[misfiled, duplicate], &home, &[]);
        let moves_of_dupe: Vec<&Step> = steps
            .iter()
            .filter(|s| s.args.str_or("from", "").ends_with("album-track-copy.mp3"))
            .collect();
        assert_eq!(moves_of_dupe.len(), 1, "one file, one destination");
        assert!(
            moves_of_dupe[0]
                .args
                .str_or("to", "")
                .contains("Duplicates"),
            "duplicate handling takes precedence over filing: {}",
            moves_of_dupe[0].args.str_or("to", "")
        );
        // The original is still filed into the library.
        assert!(steps
            .iter()
            .any(|s| s.args.str_or("to", "").ends_with("Music/album-track.mp3")));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn colliding_destinations_are_disambiguated_not_dropped() {
        let home = scratch("collide-dest");
        let f = Finding {
            kind: "screenshots",
            severity: 2,
            title: "t".into(),
            detail: "d".into(),
            paths: vec![
                home.join("Desktop/shot.png"),
                home.join("Downloads/shot.png"),
            ],
            bytes: 2,
        };
        let steps = plan_steps(&[f], &home, &[]);
        assert_eq!(steps.len(), 2, "both files should still be moved");
        let dests: Vec<String> = steps
            .iter()
            .map(|s| s.args.str_or("to", "").to_string())
            .collect();
        assert_ne!(dests[0], dests[1], "two files cannot land on the same path");
        assert!(dests[1].contains("shot (2).png"), "{}", dests[1]);
        let _ = fs::remove_dir_all(&home);
    }

    /// Write a file with a chosen modification time, so "arrived together"
    /// can be tested without waiting.
    fn touch_at(dir: &Path, name: &str, bytes: usize, mtime: u64) {
        let p = dir.join(name);
        std::fs::write(&p, vec![b'x'; bytes]).unwrap();
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime);
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(t).unwrap();
    }

    #[test]
    fn a_stem_loses_only_a_trailing_index_never_a_word() {
        assert_eq!(shared_stem("invoice-part1.pdf"), "invoice");
        assert_eq!(shared_stem("invoice-2024-part2.pdf"), "invoice-2024");
        assert_eq!(shared_stem("holiday-1.jpg"), "holiday");
        // The marker word is only stripped when something is left behind it.
        assert_eq!(shared_stem("disc 2.flac"), "disc");
        assert_eq!(shared_stem("chapter_03.txt"), "chapter");
        // A word is not an index: these keep their whole name and so will not
        // be grouped with each other.
        assert_eq!(shared_stem("report_data.csv"), "report_data");
        assert_eq!(shared_stem("summary.pdf"), "summary");
        assert_eq!(shared_stem("notes.txt"), "notes");
        // A name that is nothing but digits leaves nothing to match on.
        assert_eq!(shared_stem("12345.jpg"), "");
    }

    #[test]
    fn files_that_arrived_together_are_grouped_beside_themselves() {
        let root = scratch("together");
        let dl = root.join("Downloads");
        std::fs::create_dir_all(&dl).unwrap();
        // One delivery: shared stem, seconds apart.
        touch_at(&dl, "manual-part1.pdf", 100, 1_700_000_000);
        touch_at(&dl, "manual-part2.pdf", 100, 1_700_000_030);
        touch_at(&dl, "manual-part3.pdf", 100, 1_700_000_060);

        let found = find_arrived_together(std::slice::from_ref(&dl));
        assert_eq!(found.len(), 1, "expected one group, got {found:?}");
        assert_eq!(found[0].paths.len(), 3);
        assert!(found[0].title.contains("manual"), "{}", found[0].title);

        let steps = plan_steps(&found, &root, &[]);
        assert_eq!(steps.len(), 3);
        for st in &steps {
            let to = st.args.str_or("to", "");
            // All PDFs, so the set nests inside the type folder.
            assert!(
                to.contains("/Downloads/Documents/manual/"),
                "grouped somewhere unexpected: {to}"
            );
        }
    }

    #[test]
    fn a_shared_name_alone_is_not_a_delivery() {
        // The same stem, but a year apart. Two files called report-1 and
        // report-2 downloaded in different years are not a set, and moving
        // them into a folder together would be wrong.
        let root = scratch("apart");
        let dl = root.join("Downloads");
        std::fs::create_dir_all(&dl).unwrap();
        touch_at(&dl, "report-1.pdf", 10, 1_600_000_000);
        touch_at(&dl, "report-2.pdf", 10, 1_650_000_000);
        touch_at(&dl, "report-3.pdf", 10, 1_700_000_000);
        assert!(find_arrived_together(&[dl]).is_empty());
    }

    #[test]
    fn arriving_at_the_same_moment_is_not_a_delivery_either() {
        // Forty unrelated files from one busy afternoon share a timestamp but
        // nothing else. Both signals have to agree.
        let root = scratch("busy");
        let dl = root.join("Downloads");
        std::fs::create_dir_all(&dl).unwrap();
        for (i, name) in ["taxes.pdf", "kitten.jpg", "resume.docx", "budget.csv"]
            .iter()
            .enumerate()
        {
            touch_at(&dl, name, 10, 1_700_000_000 + i as u64);
        }
        assert!(find_arrived_together(&[dl]).is_empty());
    }

    #[test]
    fn two_files_are_a_coincidence_not_a_set() {
        let root = scratch("pair");
        let dl = root.join("Downloads");
        std::fs::create_dir_all(&dl).unwrap();
        touch_at(&dl, "scan-1.jpg", 10, 1_700_000_000);
        touch_at(&dl, "scan-2.jpg", 10, 1_700_000_010);
        assert!(find_arrived_together(&[dl]).is_empty());
    }

    #[test]
    fn loose_files_of_one_kind_are_gathered_into_a_folder_named_for_it() {
        let root = scratch("bykind");
        let dl = root.join("Downloads");
        std::fs::create_dir_all(&dl).unwrap();
        for i in 0..5 {
            // Distinct stems and spread-out times, so this is a by-kind case
            // and not a delivery.
            touch_at(
                &dl,
                &format!("photo{i}.jpg"),
                10,
                1_700_000_000 + i * 100_000,
            );
        }
        let found = find_loose_by_kind(std::slice::from_ref(&dl));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].kind, "loose_by_kind");

        let steps = plan_steps(&found, &root, &[]);
        assert_eq!(steps.len(), 5);
        for st in &steps {
            let to = st.args.str_or("to", "");
            assert!(to.contains("/Downloads/Images/"), "went to {to}");
        }
    }

    #[test]
    fn a_few_loose_files_are_left_alone() {
        // Three photos do not need a folder; making one is not tidying.
        let root = scratch("fewkind");
        let dl = root.join("Downloads");
        std::fs::create_dir_all(&dl).unwrap();
        for i in 0..3 {
            touch_at(
                &dl,
                &format!("photo{i}.jpg"),
                10,
                1_700_000_000 + i * 100_000,
            );
        }
        assert!(find_loose_by_kind(&[dl]).is_empty());
    }

    #[test]
    fn only_downloads_and_documents_get_reorganised() {
        // Somebody's project folder is not clutter; its layout is the point.
        let root = scratch("scope");
        let proj = root.join("Projects");
        std::fs::create_dir_all(&proj).unwrap();
        for i in 0..6 {
            touch_at(
                &proj,
                &format!("asset{i}.png"),
                10,
                1_700_000_000 + i * 100_000,
            );
        }
        assert!(find_loose_by_kind(std::slice::from_ref(&proj)).is_empty());
        assert!(find_arrived_together(&[proj]).is_empty());
    }

    #[test]
    fn a_set_stays_together_inside_its_type_folder() {
        // Type organises the result; a delivery keeps its files together
        // within that. Both rules match here and the outcome honours both.
        let root = scratch("beats");
        let dl = root.join("Downloads");
        std::fs::create_dir_all(&dl).unwrap();
        for i in 1..=5 {
            touch_at(&dl, &format!("wedding-{i}.jpg"), 10, 1_700_000_000 + i);
        }
        let mut findings = find_arrived_together(std::slice::from_ref(&dl));
        findings.extend(find_loose_by_kind(std::slice::from_ref(&dl)));
        assert_eq!(findings.len(), 2, "both rules should match here");

        let steps = plan_steps(&findings, &root, &[]);
        assert_eq!(steps.len(), 5, "each file moved exactly once");
        for st in &steps {
            let to = st.args.str_or("to", "");
            assert!(
                to.contains("/Downloads/Images/wedding/"),
                "the set should nest inside its type folder: {to}"
            );
        }
    }

    #[test]
    fn already_filed_things_are_left_where_they_are() {
        // Files inside a subfolder have been filed already, and a dotfile is
        // the browser's business.
        let root = scratch("filed");
        let dl = root.join("Downloads");
        let sub = dl.join("Images");
        std::fs::create_dir_all(&sub).unwrap();
        for i in 0..6 {
            touch_at(
                &sub,
                &format!("photo{i}.jpg"),
                10,
                1_700_000_000 + i * 100_000,
            );
        }
        for i in 0..6 {
            touch_at(
                &dl,
                &format!(".partial{i}.jpg"),
                10,
                1_700_000_000 + i * 100_000,
            );
        }
        assert!(find_loose_by_kind(&[dl]).is_empty());
    }

    #[test]
    fn informational_findings_produce_no_steps() {
        let home = scratch("informational");
        let big = Finding {
            kind: "large_files",
            severity: 1,
            title: "t".into(),
            detail: "d".into(),
            paths: vec![home.join("huge.iso")],
            bytes: 5_000_000_000,
        };
        let empty = Finding {
            kind: "empty_dirs",
            severity: 1,
            title: "t".into(),
            detail: "d".into(),
            paths: vec![home.join("nothing")],
            bytes: 0,
        };
        assert!(
            plan_steps(&[big, empty], &home, &[]).is_empty(),
            "the curator must not act on things it cannot judge"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn kind_filter_narrows_the_proposal() {
        let home = scratch("filter");
        let media = Finding {
            kind: "misfiled_media",
            severity: 3,
            title: "t".into(),
            detail: "d".into(),
            paths: vec![home.join("Downloads/a.mp3")],
            bytes: 1,
        };
        let shots = Finding {
            kind: "screenshots",
            severity: 2,
            title: "t".into(),
            detail: "d".into(),
            paths: vec![home.join("Desktop/screenshot1.png")],
            bytes: 1,
        };
        let all = plan_steps(&[media.clone(), shots.clone()], &home, &[]);
        assert_eq!(all.len(), 2);
        let only = plan_steps(&[media, shots], &home, &["screenshots".to_string()]);
        assert_eq!(only.len(), 1);
        assert!(only[0].args.str_or("to", "").contains("Screenshots"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn stale_downloads_need_a_meaningful_number_to_report() {
        let dir = scratch("stale");
        let downloads = dir.join("Downloads");
        fs::create_dir_all(&downloads).unwrap();
        fs::write(downloads.join("recent.txt"), b"x").unwrap();
        // Only one file, and it is new: nothing worth mentioning.
        assert!(find_stale_downloads(&[downloads], now_secs()).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn findings_sort_by_consequence() {
        let dir = scratch("sort");
        fs::create_dir_all(dir.join("Downloads")).unwrap();
        for i in 0..6 {
            fs::write(
                dir.join("Downloads").join(format!("screenshot{}.png", i)),
                b"x",
            )
            .unwrap();
        }
        fs::write(dir.join("Downloads/track.mp3"), vec![1u8; 5000]).unwrap();
        let out = analyse(&[dir.join("Downloads")], &[], now_secs());
        assert!(!out.is_empty());
        for pair in out.windows(2) {
            assert!(
                pair[0].severity >= pair[1].severity,
                "findings must be ordered by severity"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn formats_byte_counts_readably() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
