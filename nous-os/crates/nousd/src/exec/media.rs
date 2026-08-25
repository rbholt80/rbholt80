//! Media executor: library, playback, and non-destructive editing.
//!
//! Editing is **an edit graph, not a file mutation**. `media.edit` builds and
//! amends a declarative timeline; `media.render` compiles that timeline into an
//! ffmpeg invocation and writes a *new* file. A source clip is never modified,
//! which is what makes "cut the first thirty seconds" a safe thing to say out
//! loud to a computer.

use super::{Effect, ExecCtx};
use nous_core::cap::Capability;
use nous_core::journal::Undo;
use nous_core::json::{json_obj, parse, Json};
use nous_core::Step;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::sysops::{have, run};

pub fn execute(cap: &Capability, step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    match cap.action.as_str() {
        "probe" => probe_step(step),
        "index" => index(step, ctx),
        "search" => search(step, ctx),
        "thumbnail" => thumbnail(step, ctx),
        "play" => play(step, ctx),
        "control" => control(step, ctx),
        "edit" => edit(step, ctx),
        "render" => render(step, ctx),
        other => Err(format!("media executor cannot '{}'", other)),
    }
}

fn arg_path(step: &Step, key: &str) -> Result<PathBuf, String> {
    let raw = step
        .args
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("step is missing the '{}' argument", key))?;
    Ok(nous_core::config::expand_tilde(raw))
}

// ------------------------------------------------------------------- probing

/// Read a file's media properties with ffprobe.
pub fn probe(path: &Path) -> Result<Json, String> {
    if !have("ffprobe") {
        return Err("ffprobe is not installed (install the ffmpeg package)".to_string());
    }
    let p = path.to_string_lossy().to_string();
    let out = run(
        "ffprobe",
        &[
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            &p,
        ],
        Duration::from_secs(30),
    )?;
    out.require("ffprobe")?;
    let raw = parse(&out.stdout).map_err(|e| format!("ffprobe returned unreadable JSON: {}", e))?;

    let format = raw.get("format").cloned().unwrap_or_else(Json::obj);
    let streams = raw.arr_or_empty("streams");
    let video = streams
        .iter()
        .find(|s| s.str_or("codec_type", "") == "video");
    let audio = streams
        .iter()
        .find(|s| s.str_or("codec_type", "") == "audio");

    let duration: f64 = format.str_or("duration", "0").parse().unwrap_or(0.0);
    let tags = format.get("tags").cloned().unwrap_or_else(Json::obj);

    Ok(json_obj([
        ("path", p.into()),
        ("duration", duration.into()),
        (
            "size",
            format
                .str_or("size", "0")
                .parse::<f64>()
                .unwrap_or(0.0)
                .into(),
        ),
        ("format", format.str_or("format_name", "").into()),
        ("has_video", video.is_some().into()),
        ("has_audio", audio.is_some().into()),
        (
            "width",
            video.map(|v| v.f64_or("width", 0.0)).unwrap_or(0.0).into(),
        ),
        (
            "height",
            video.map(|v| v.f64_or("height", 0.0)).unwrap_or(0.0).into(),
        ),
        (
            "fps",
            video
                .map(|v| parse_rational(v.str_or("r_frame_rate", "0/1")))
                .unwrap_or(0.0)
                .into(),
        ),
        (
            "vcodec",
            video
                .map(|v| v.str_or("codec_name", "").to_string())
                .unwrap_or_default()
                .into(),
        ),
        (
            "acodec",
            audio
                .map(|a| a.str_or("codec_name", "").to_string())
                .unwrap_or_default()
                .into(),
        ),
        // Tag keys vary in case between containers; look for both spellings.
        ("title", tag(&tags, "title").into()),
        ("artist", tag(&tags, "artist").into()),
        ("album", tag(&tags, "album").into()),
        ("date", tag(&tags, "date").into()),
    ]))
}

fn tag(tags: &Json, key: &str) -> String {
    tags.get(key)
        .or_else(|| tags.get(&key.to_uppercase()))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// ffprobe reports frame rates as `30000/1001`.
pub fn parse_rational(s: &str) -> f64 {
    match s.split_once('/') {
        Some((n, d)) => {
            let (n, d): (f64, f64) = (n.parse().unwrap_or(0.0), d.parse().unwrap_or(1.0));
            if d == 0.0 {
                0.0
            } else {
                (n / d * 100.0).round() / 100.0
            }
        }
        None => s.parse().unwrap_or(0.0),
    }
}

fn probe_step(step: &Step) -> Result<Effect, String> {
    let path = arg_path(step, "path")?;
    let info = probe(&path)?;
    let d = info.f64_or("duration", 0.0);
    Ok(Effect::read_only(
        info,
        format!("probed {} ({})", path.display(), fmt_duration(d)),
    ))
}

pub fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

// ------------------------------------------------------------------- library

fn library_path() -> PathBuf {
    nous_core::ipc::state_dir().join("media/library.json")
}

fn index(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let roots: Vec<PathBuf> = if let Some(list) = step.args.get("roots") {
        list.as_arr()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(nous_core::config::expand_tilde)
                    .collect()
            })
            .unwrap_or_default()
    } else {
        default_media_roots(ctx)
    };
    let exclude = ctx.cfg.list("index.exclude");
    let deep = step.args.bool_or("probe", false);

    let mut files = Vec::new();
    for root in &roots {
        super::fsops::walk(root, 8, &exclude, &mut files, 50_000);
    }

    let mut items = Vec::new();
    for f in &files {
        let kind = super::fsops::classify(f, false);
        if kind != "audio" && kind != "video" {
            continue;
        }
        let md = std::fs::metadata(f).ok();
        let mut entry = json_obj([
            ("path", f.to_string_lossy().to_string().into()),
            (
                "name",
                f.file_name().and_then(|s| s.to_str()).unwrap_or("").into(),
            ),
            ("kind", kind.into()),
            ("size", md.as_ref().map(|m| m.len()).unwrap_or(0).into()),
            (
                "modified",
                md.as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
                    .into(),
            ),
        ]);
        // Probing every file is expensive; it is opt-in and best-effort.
        if deep {
            if let Ok(info) = probe(f) {
                for key in ["duration", "title", "artist", "album", "width", "height"] {
                    if let Some(v) = info.get(key) {
                        entry.set(key, v.clone());
                    }
                }
            }
        }
        items.push(entry);
    }

    let count = items.len();
    let audio = items
        .iter()
        .filter(|i| i.str_or("kind", "") == "audio")
        .count();
    let video = count - audio;

    if !ctx.dry_run {
        let lib = json_obj([
            ("version", 1u64.into()),
            ("updated", nous_core::journal::now_secs().into()),
            ("items", Json::Arr(items.clone())),
        ]);
        let path = library_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create media dir: {}", e))?;
        }
        std::fs::write(&path, lib.to_string())
            .map_err(|e| format!("cannot write media library: {}", e))?;
    }

    Ok(Effect::read_only(
        json_obj([
            ("count", count.into()),
            ("audio", audio.into()),
            ("video", video.into()),
        ]),
        format!(
            "indexed {} media files ({} audio, {} video)",
            count, audio, video
        ),
    ))
}

pub fn default_media_roots(_ctx: &ExecCtx) -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home".to_string());
    ["Music", "Videos", "Movies", "Downloads", "Pictures"]
        .iter()
        .map(|d| PathBuf::from(&home).join(d))
        .filter(|p| p.exists())
        .collect()
}

pub fn load_library() -> Json {
    std::fs::read_to_string(library_path())
        .ok()
        .and_then(|s| parse(&s).ok())
        .unwrap_or_else(|| json_obj([("items", Json::Arr(vec![]))]))
}

fn search(step: &Step, _ctx: &ExecCtx) -> Result<Effect, String> {
    let q = step.args.str_or("query", "").to_ascii_lowercase();
    let kind = step.args.str_or("kind", "").to_string();
    let limit = step
        .args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(60) as usize;

    let lib = load_library();
    let mut hits: Vec<Json> = lib
        .arr_or_empty("items")
        .into_iter()
        .filter(|i| kind.is_empty() || i.str_or("kind", "") == kind)
        .filter(|i| {
            if q.is_empty() {
                return true;
            }
            let hay = format!(
                "{} {} {} {}",
                i.str_or("name", ""),
                i.str_or("title", ""),
                i.str_or("artist", ""),
                i.str_or("album", "")
            )
            .to_ascii_lowercase();
            q.split_whitespace().all(|term| hay.contains(term))
        })
        .collect();

    hits.sort_by(|a, b| {
        b.f64_or("modified", 0.0)
            .partial_cmp(&a.f64_or("modified", 0.0))
            .unwrap()
    });
    let total = hits.len();
    hits.truncate(limit);
    Ok(Effect::read_only(
        json_obj([("items", Json::Arr(hits)), ("total", total.into())]),
        format!("{} media matches for '{}'", total, q),
    ))
}

fn thumbnail(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let path = arg_path(step, "path")?;
    let at = step.args.f64_or("at", 3.0);
    if !have("ffmpeg") {
        return Err("ffmpeg is not installed".to_string());
    }
    let cache = nous_core::ipc::state_dir().join("media/thumbs");
    std::fs::create_dir_all(&cache).map_err(|e| format!("cannot create thumb cache: {}", e))?;
    let out = cache.join(format!("{}.jpg", stable_key(&path)));

    if out.exists() {
        return Ok(Effect::read_only(
            json_obj([
                ("thumbnail", out.to_string_lossy().to_string().into()),
                ("cached", true.into()),
            ]),
            "thumbnail already cached",
        ));
    }
    if ctx.dry_run {
        return Ok(Effect::read_only(Json::obj(), "would generate a thumbnail"));
    }

    let src = path.to_string_lossy().to_string();
    let dst = out.to_string_lossy().to_string();
    let at_s = format!("{}", at);
    let r = run(
        "ffmpeg",
        &[
            "-nostdin",
            "-y",
            "-ss",
            &at_s,
            "-i",
            &src,
            "-frames:v",
            "1",
            "-vf",
            "scale=480:-1",
            &dst,
        ],
        Duration::from_secs(45),
    )?;
    r.require("ffmpeg")?;
    Ok(Effect::read_only(
        json_obj([("thumbnail", dst.into()), ("cached", false.into())]),
        format!("made a thumbnail of {}", path.display()),
    ))
}

/// A filename-safe, stable key for a path. FNV-1a: short, fast, and this is a
/// cache key, not a security boundary.
pub fn stable_key(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.to_string_lossy().as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", hash)
}

// ------------------------------------------------------------------ playback

fn mpv_socket() -> PathBuf {
    nous_core::ipc::state_dir().join("media/mpv.sock")
}

fn play(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let path = arg_path(step, "path")?;
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    if ctx.dry_run {
        return Ok(Effect::read_only(
            json_obj([("path", path.to_string_lossy().to_string().into())]),
            format!("would play {}", path.display()),
        ));
    }
    if !have("mpv") {
        return Err("mpv is not installed (install the mpv package)".to_string());
    }
    let sock = mpv_socket();
    if let Some(d) = sock.parent() {
        std::fs::create_dir_all(d).ok();
    }
    // Detach: playback outlives the request that started it.
    let child = std::process::Command::new("mpv")
        .arg(format!("--input-ipc-server={}", sock.display()))
        .arg("--force-window=yes")
        .arg("--idle=yes")
        .arg(path.as_os_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start mpv: {}", e))?;

    Ok(Effect::with_undo(
        json_obj([
            ("path", path.to_string_lossy().to_string().into()),
            ("pid", (child.id() as u64).into()),
        ]),
        Undo::Manual {
            note: "stop playback".to_string(),
        },
        format!("playing {}", path.display()),
    ))
}

fn control(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let action = step.args.str_or("action", "").to_string();
    let command: Vec<Json> = match action.as_str() {
        "pause" => vec!["set_property".into(), "pause".into(), Json::Bool(true)],
        "resume" => vec!["set_property".into(), "pause".into(), Json::Bool(false)],
        "toggle" => vec!["cycle".into(), "pause".into()],
        "stop" => vec!["quit".into()],
        "next" => vec!["playlist-next".into()],
        "previous" => vec!["playlist-prev".into()],
        "seek" => vec![
            "seek".into(),
            Json::Num(step.args.f64_or("seconds", 10.0)),
            "relative".into(),
        ],
        "volume" => vec![
            "set_property".into(),
            "volume".into(),
            Json::Num(step.args.f64_or("level", 70.0)),
        ],
        other => return Err(format!("unknown playback action '{}'", other)),
    };
    if ctx.dry_run {
        return Ok(Effect::read_only(
            Json::obj(),
            format!("would {} playback", action),
        ));
    }
    mpv_command(Json::Arr(command))?;
    Ok(Effect::read_only(
        json_obj([("action", action.clone().into())]),
        format!("playback: {}", action),
    ))
}

/// Send one JSON-IPC command to a running mpv.
fn mpv_command(command: Json) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    let sock = mpv_socket();
    let mut s = UnixStream::connect(&sock)
        .map_err(|_| "nothing is playing (no mpv instance to control)".to_string())?;
    let msg = format!("{}\n", json_obj([("command", command)]));
    s.write_all(msg.as_bytes())
        .map_err(|e| format!("cannot talk to mpv: {}", e))
}

// ------------------------------------------------------- non-destructive edit

fn projects_dir() -> PathBuf {
    nous_core::ipc::state_dir().join("media/projects")
}

pub fn project_path(name: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    projects_dir().join(format!("{}.glyphedit.json", safe))
}

pub fn load_project(name: &str) -> Json {
    std::fs::read_to_string(project_path(name))
        .ok()
        .and_then(|s| parse(&s).ok())
        .unwrap_or_else(|| {
            json_obj([
                ("version", 1u64.into()),
                ("name", name.into()),
                ("clips", Json::Arr(vec![])),
                ("output", json_obj([("format", "mp4".into())])),
            ])
        })
}

/// Amend an edit graph. Each operation is additive and the document is written
/// whole, so the journal's file snapshot is a complete previous version of the
/// timeline — undo restores the edit, not just the file.
fn edit(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let name = step.args.str_or("project", "untitled").to_string();
    let op = step.args.str_or("op", "").to_string();
    let mut project = load_project(&name);
    let mut clips = project.arr_or_empty("clips");

    let described = match op.as_str() {
        "append" => {
            let src = arg_path(step, "path")?;
            if !src.exists() {
                return Err(format!("{} does not exist", src.display()));
            }
            let duration = probe(&src)
                .map(|p| p.f64_or("duration", 0.0))
                .unwrap_or(0.0);
            let id = format!("c{}", clips.len() + 1);
            clips.push(json_obj([
                ("id", id.clone().into()),
                ("path", src.to_string_lossy().to_string().into()),
                ("in", 0.0.into()),
                ("out", duration.into()),
                ("speed", 1.0.into()),
                ("volume", 1.0.into()),
            ]));
            format!("added {} to '{}'", src.display(), name)
        }
        "trim" => {
            let id = step.args.str_or("clip", "").to_string();
            let start = step.args.f64_or("in", 0.0);
            let end = step.args.get("out").and_then(|v| v.as_f64());
            let clip = clips
                .iter_mut()
                .find(|c| c.str_or("id", "") == id || id.is_empty())
                .ok_or_else(|| format!("no clip '{}' in '{}'", id, name))?;
            clip.set("in", start.into());
            if let Some(e) = end {
                clip.set("out", e.into());
            }
            format!(
                "trimmed clip to {}–{}",
                fmt_duration(start),
                end.map(fmt_duration).unwrap_or_else(|| "end".into())
            )
        }
        "speed" | "volume" | "fade_in" | "fade_out" => {
            let id = step.args.str_or("clip", "").to_string();
            let value = step.args.f64_or("value", 1.0);
            let clip = clips
                .iter_mut()
                .find(|c| c.str_or("id", "") == id || id.is_empty())
                .ok_or_else(|| format!("no clip '{}' in '{}'", id, name))?;
            clip.set(&op, value.into());
            format!("set {} to {} on '{}'", op, value, name)
        }
        "remove" => {
            let id = step.args.str_or("clip", "").to_string();
            let before = clips.len();
            clips.retain(|c| c.str_or("id", "") != id);
            if clips.len() == before {
                return Err(format!("no clip '{}' in '{}'", id, name));
            }
            format!("removed clip {} from '{}'", id, name)
        }
        "" => return Err("step is missing 'op'".to_string()),
        other => return Err(format!("unknown edit operation '{}'", other)),
    };

    project.set("clips", Json::Arr(clips));
    project.set("updated", nous_core::journal::now_secs().into());

    let path = project_path(&name);
    if ctx.dry_run {
        return Ok(Effect::read_only(
            project,
            format!("would have {}", described),
        ));
    }
    std::fs::create_dir_all(projects_dir())
        .map_err(|e| format!("cannot create projects dir: {}", e))?;
    let backup = ctx.journal.snapshot(&path)?;
    let existed = backup.is_some();
    std::fs::write(&path, project.to_string_pretty())
        .map_err(|e| format!("cannot save project: {}", e))?;

    Ok(Effect::with_undo(
        project,
        Undo::RestoreFile {
            path: path.to_string_lossy().to_string(),
            backup,
            existed,
        },
        described,
    ))
}

/// Compile an edit graph into ffmpeg arguments.
///
/// Split out from `render` so the compiler can be tested without ffmpeg present
/// — the argument vector is the interesting artefact, not the encode.
pub fn compile(project: &Json, output: &Path) -> Result<Vec<String>, String> {
    let clips = project.arr_or_empty("clips");
    if clips.is_empty() {
        return Err("this project has no clips to render".to_string());
    }

    let mut args: Vec<String> = vec!["-nostdin".into(), "-y".into()];
    for clip in &clips {
        args.push("-i".into());
        args.push(clip.str_or("path", "").to_string());
    }

    // One filter chain per clip, then a concat across all of them.
    let mut chains: Vec<String> = Vec::new();
    let mut concat_inputs = String::new();
    for (i, clip) in clips.iter().enumerate() {
        let start = clip.f64_or("in", 0.0);
        let end = clip.f64_or("out", 0.0);
        let speed = clip.f64_or("speed", 1.0).max(0.05);
        let volume = clip.f64_or("volume", 1.0).max(0.0);
        let fade_in = clip.f64_or("fade_in", 0.0);
        let fade_out = clip.f64_or("fade_out", 0.0);
        if end > 0.0 && end <= start {
            return Err(format!(
                "clip {} ends before it starts",
                clip.str_or("id", "?")
            ));
        }
        let dur = if end > 0.0 { end - start } else { 0.0 };

        let mut v = format!("[{}:v]trim=start={}", i, start);
        if dur > 0.0 {
            v.push_str(&format!(":duration={}", dur));
        }
        v.push_str(",setpts=PTS-STARTPTS");
        if (speed - 1.0).abs() > f64::EPSILON {
            v.push_str(&format!(",setpts={}*PTS", 1.0 / speed));
        }
        if fade_in > 0.0 {
            v.push_str(&format!(",fade=t=in:st=0:d={}", fade_in));
        }
        if fade_out > 0.0 && dur > fade_out {
            v.push_str(&format!(
                ",fade=t=out:st={}:d={}",
                (dur / speed) - fade_out,
                fade_out
            ));
        }
        v.push_str(&format!("[v{}]", i));
        chains.push(v);

        let mut a = format!("[{}:a]atrim=start={}", i, start);
        if dur > 0.0 {
            a.push_str(&format!(":duration={}", dur));
        }
        a.push_str(",asetpts=PTS-STARTPTS");
        if (speed - 1.0).abs() > f64::EPSILON {
            // atempo is only valid in [0.5, 2.0]; chain stages to reach further.
            for stage in atempo_chain(speed) {
                a.push_str(&format!(",atempo={}", stage));
            }
        }
        if (volume - 1.0).abs() > f64::EPSILON {
            a.push_str(&format!(",volume={}", volume));
        }
        a.push_str(&format!("[a{}]", i));
        chains.push(a);

        concat_inputs.push_str(&format!("[v{}][a{}]", i, i));
    }

    chains.push(format!(
        "{}concat=n={}:v=1:a=1[vout][aout]",
        concat_inputs,
        clips.len()
    ));

    args.push("-filter_complex".into());
    args.push(chains.join(";"));
    args.push("-map".into());
    args.push("[vout]".into());
    args.push("-map".into());
    args.push("[aout]".into());

    let out_cfg = project.get("output").cloned().unwrap_or_else(Json::obj);
    args.push("-c:v".into());
    args.push(out_cfg.str_or("vcodec", "libx264").to_string());
    args.push("-preset".into());
    args.push(out_cfg.str_or("preset", "medium").to_string());
    args.push("-crf".into());
    args.push(format!("{}", out_cfg.f64_or("crf", 20.0) as i64));
    args.push("-c:a".into());
    args.push(out_cfg.str_or("acodec", "aac").to_string());
    args.push(output.to_string_lossy().to_string());
    Ok(args)
}

/// Decompose a speed factor into atempo stages each within ffmpeg's valid range.
pub fn atempo_chain(speed: f64) -> Vec<f64> {
    let mut remaining = speed;
    let mut stages = Vec::new();
    while remaining > 2.0 {
        stages.push(2.0);
        remaining /= 2.0;
    }
    while remaining < 0.5 {
        stages.push(0.5);
        remaining /= 0.5;
    }
    if (remaining - 1.0).abs() > f64::EPSILON {
        stages.push((remaining * 1000.0).round() / 1000.0);
    }
    stages
}

fn render(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let name = step.args.str_or("project", "untitled").to_string();
    let project = load_project(&name);
    let output = match step.args.get("output").and_then(|v| v.as_str()) {
        Some(o) => nous_core::config::expand_tilde(o),
        None => {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home)
                .join("Videos")
                .join(format!("{}.mp4", name))
        }
    };
    if output.exists() {
        return Err(format!(
            "{} already exists — choose another name",
            output.display()
        ));
    }
    let args = compile(&project, &output)?;

    if ctx.dry_run {
        return Ok(Effect::read_only(
            json_obj([
                ("output", output.to_string_lossy().to_string().into()),
                (
                    "ffmpeg_args",
                    Json::Arr(args.iter().map(|a| Json::Str(a.clone())).collect()),
                ),
            ]),
            format!("would render '{}' to {}", name, output.display()),
        ));
    }
    if !have("ffmpeg") {
        return Err("ffmpeg is not installed".to_string());
    }
    if let Some(d) = output.parent() {
        std::fs::create_dir_all(d).ok();
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = run("ffmpeg", &refs, Duration::from_secs(3600))?;
    out.require("ffmpeg")?;

    Ok(Effect::with_undo(
        json_obj([
            ("output", output.to_string_lossy().to_string().into()),
            ("project", name.clone().into()),
        ]),
        // Rendering only ever creates a new file, so undo is a clean removal.
        Undo::RestoreFile {
            path: output.to_string_lossy().to_string(),
            backup: None,
            existed: false,
        },
        format!("rendered '{}' to {}", name, output.display()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(path: &str, start: f64, end: f64) -> Json {
        json_obj([
            ("id", "c1".into()),
            ("path", path.into()),
            ("in", start.into()),
            ("out", end.into()),
        ])
    }

    #[test]
    fn compiles_a_single_trimmed_clip() {
        let p = json_obj([("clips", Json::Arr(vec![clip("/m/a.mp4", 30.0, 90.0)]))]);
        let args = compile(&p, Path::new("/out/final.mp4")).unwrap();
        let joined = args.join(" ");
        assert!(joined.contains("-i /m/a.mp4"));
        assert!(joined.contains("trim=start=30:duration=60"), "{joined}");
        assert!(joined.contains("concat=n=1:v=1:a=1"));
        assert_eq!(args.last().unwrap(), "/out/final.mp4");
    }

    #[test]
    fn compiles_a_multi_clip_concat() {
        let p = json_obj([(
            "clips",
            Json::Arr(vec![
                clip("/m/a.mp4", 0.0, 10.0),
                clip("/m/b.mp4", 5.0, 15.0),
            ]),
        )]);
        let args = compile(&p, Path::new("/out/x.mp4")).unwrap();
        let joined = args.join(" ");
        assert!(joined.contains("[v0][a0][v1][a1]concat=n=2"), "{joined}");
        assert_eq!(args.iter().filter(|a| *a == "-i").count(), 2);
    }

    #[test]
    fn speed_changes_adjust_both_video_and_audio() {
        let mut c = clip("/m/a.mp4", 0.0, 60.0);
        c.set("speed", 2.0.into());
        let p = json_obj([("clips", Json::Arr(vec![c]))]);
        let joined = compile(&p, Path::new("/o.mp4")).unwrap().join(" ");
        assert!(joined.contains("setpts=0.5*PTS"), "{joined}");
        assert!(joined.contains("atempo=2"), "{joined}");
    }

    #[test]
    fn extreme_speeds_are_split_into_valid_atempo_stages() {
        // ffmpeg rejects atempo outside [0.5, 2.0], so 8x must become 2*2*2.
        assert_eq!(atempo_chain(8.0), vec![2.0, 2.0, 2.0]);
        assert_eq!(atempo_chain(0.25), vec![0.5, 0.5]);
        assert_eq!(atempo_chain(1.5), vec![1.5]);
        assert!(atempo_chain(1.0).is_empty(), "1x needs no filter at all");
    }

    #[test]
    fn fades_are_placed_relative_to_clip_duration() {
        let mut c = clip("/m/a.mp4", 0.0, 20.0);
        c.set("fade_in", 2.0.into());
        c.set("fade_out", 3.0.into());
        let p = json_obj([("clips", Json::Arr(vec![c]))]);
        let joined = compile(&p, Path::new("/o.mp4")).unwrap().join(" ");
        assert!(joined.contains("fade=t=in:st=0:d=2"), "{joined}");
        assert!(joined.contains("fade=t=out:st=17:d=3"), "{joined}");
    }

    #[test]
    fn rejects_an_empty_or_inverted_timeline() {
        let empty = json_obj([("clips", Json::Arr(vec![]))]);
        assert!(compile(&empty, Path::new("/o.mp4")).is_err());

        let bad = json_obj([("clips", Json::Arr(vec![clip("/m/a.mp4", 30.0, 10.0)]))]);
        let err = compile(&bad, Path::new("/o.mp4")).unwrap_err();
        assert!(err.contains("ends before it starts"), "{err}");
    }

    #[test]
    fn parses_ffprobe_rationals() {
        assert_eq!(parse_rational("30000/1001"), 29.97);
        assert_eq!(parse_rational("25/1"), 25.0);
        assert_eq!(parse_rational("0/0"), 0.0);
    }

    #[test]
    fn formats_durations_for_humans() {
        assert_eq!(fmt_duration(65.0), "1:05");
        assert_eq!(fmt_duration(3725.0), "1:02:05");
        assert_eq!(fmt_duration(-4.0), "0:00");
    }

    #[test]
    fn project_names_cannot_escape_the_projects_directory() {
        let p = project_path("../../etc/passwd");
        assert!(!p.to_string_lossy().contains(".."), "{}", p.display());
        assert!(p.starts_with(projects_dir()));
    }

    #[test]
    fn thumbnail_keys_are_stable_and_distinct() {
        let a = stable_key(Path::new("/m/a.mp4"));
        assert_eq!(a, stable_key(Path::new("/m/a.mp4")));
        assert_ne!(a, stable_key(Path::new("/m/b.mp4")));
        assert_eq!(a.len(), 16);
    }
}
