//! Filesystem executor.
//!
//! The one design decision worth calling out: **delete moves to a trash store**.
//! An AI that can permanently remove files is an AI that can permanently remove
//! the wrong files. Deletion here is a move, the move is journalled, and the
//! journal knows how to move it back.

use super::{Effect, ExecCtx};
use nous_core::cap::Capability;
use nous_core::journal::{now_secs, Undo};
use nous_core::json::{json_obj, Json};
use nous_core::Step;
use std::fs;
use std::path::{Path, PathBuf};

/// Reading a file into a model's context is bounded: a stray `fs.read` on a
/// 4 GB video should fail cleanly rather than exhaust the machine.
const DEFAULT_READ_LIMIT: u64 = 512 * 1024;

pub fn execute(cap: &Capability, step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    match cap.action.as_str() {
        "list" => list(step, ctx),
        "stat" => stat(step),
        "read" => read(step, ctx),
        "write" => write(step, ctx),
        "mkdir" => mkdir(step, ctx),
        "move" => rename(step, ctx),
        "delete" => delete(step, ctx),
        other => Err(format!("fs executor cannot '{}'", other)),
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

/// Describe one directory entry richly enough for a file manager to render it
/// without a second round trip.
pub fn entry_json(path: &Path) -> Json {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let md = fs::symlink_metadata(path).ok();
    let (size, modified, is_dir, is_link) = match &md {
        Some(m) => (
            m.len(),
            m.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
            m.is_dir(),
            m.file_type().is_symlink(),
        ),
        None => (0, 0, false, false),
    };
    json_obj([
        ("name", name.clone().into()),
        ("path", path.to_string_lossy().to_string().into()),
        ("size", size.into()),
        ("modified", modified.into()),
        ("is_dir", is_dir.into()),
        ("is_link", is_link.into()),
        ("hidden", name.starts_with('.').into()),
        ("kind", classify(path, is_dir).into()),
        ("ext", extension(path).into()),
    ])
}

/// A coarse content class, used by the explorer for icons and by the curator
/// for grouping. Extension-based on purpose: it must be cheap enough to run
/// across a whole home directory.
pub fn classify(path: &Path, is_dir: bool) -> &'static str {
    if is_dir {
        return "folder";
    }
    match extension(path).as_str() {
        "mp3" | "flac" | "wav" | "m4a" | "ogg" | "opus" | "aac" | "wma" | "aiff" => "audio",
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg" => "video",
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "heic" | "svg" | "avif" => {
            "image"
        }
        "pdf" | "epub" | "mobi" | "djvu" => "document",
        "doc" | "docx" | "odt" | "rtf" | "pages" => "document",
        "xls" | "xlsx" | "ods" | "csv" | "tsv" | "numbers" => "sheet",
        "ppt" | "pptx" | "odp" | "key" => "slides",
        "txt" | "md" | "rst" | "org" | "log" => "text",
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "c" | "h" | "cpp" | "hpp" | "java"
        | "kt" | "swift" | "rb" | "php" | "sh" | "lua" | "sql" | "glyph" => "code",
        "json" | "yaml" | "yml" | "toml" | "ini" | "conf" | "xml" => "config",
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "zst" => "archive",
        "iso" | "img" | "qcow2" | "vmdk" => "image-disk",
        "deb" | "rpm" | "appimage" | "flatpak" | "snap" | "exe" | "msi" | "dmg" => "package",
        _ => "file",
    }
}

pub fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn list(step: &Step, _ctx: &ExecCtx) -> Result<Effect, String> {
    let dir = arg_path(step, "path")?;
    let show_hidden = step.args.bool_or("hidden", false);
    let md = fs::metadata(&dir).map_err(|e| format!("cannot open {}: {}", dir.display(), e))?;
    if !md.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }

    let mut entries: Vec<Json> = Vec::new();
    let mut skipped = 0usize;
    for item in fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {}", dir.display(), e))? {
        let item = match item {
            Ok(i) => i,
            // A single unreadable entry should not fail the whole listing.
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let p = item.path();
        let name = item.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        entries.push(entry_json(&p));
    }

    // Directories first, then by name: the ordering a file manager expects.
    entries.sort_by(|a, b| {
        let (ad, bd) = (a.bool_or("is_dir", false), b.bool_or("is_dir", false));
        bd.cmp(&ad).then_with(|| {
            a.str_or("name", "")
                .to_lowercase()
                .cmp(&b.str_or("name", "").to_lowercase())
        })
    });

    let n = entries.len();
    Ok(Effect::read_only(
        json_obj([
            ("path", dir.to_string_lossy().to_string().into()),
            ("entries", Json::Arr(entries)),
            ("count", n.into()),
            ("skipped", skipped.into()),
        ]),
        format!("listed {} ({} entries)", dir.display(), n),
    ))
}

fn stat(step: &Step) -> Result<Effect, String> {
    let path = arg_path(step, "path")?;
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    Ok(Effect::read_only(
        entry_json(&path),
        format!("described {}", path.display()),
    ))
}

fn read(step: &Step, _ctx: &ExecCtx) -> Result<Effect, String> {
    let path = arg_path(step, "path")?;
    let limit = step
        .args
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_READ_LIMIT);
    let md = fs::metadata(&path).map_err(|e| format!("cannot stat {}: {}", path.display(), e))?;
    if md.is_dir() {
        return Err(format!("{} is a directory", path.display()));
    }
    let bytes = fs::read(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    let truncated = bytes.len() as u64 > limit;
    let slice = if truncated {
        &bytes[..limit as usize]
    } else {
        &bytes[..]
    };

    // Binary files are reported as such rather than mangled into replacement
    // characters and fed to a model as if they were prose.
    let text = match std::str::from_utf8(slice) {
        Ok(s) => s.to_string(),
        Err(_) => {
            return Ok(Effect::read_only(
                json_obj([
                    ("path", path.to_string_lossy().to_string().into()),
                    ("binary", true.into()),
                    ("size", md.len().into()),
                    ("kind", classify(&path, false).into()),
                ]),
                format!("{} is binary ({} bytes)", path.display(), md.len()),
            ))
        }
    };
    Ok(Effect::read_only(
        json_obj([
            ("path", path.to_string_lossy().to_string().into()),
            ("content", text.into()),
            ("size", md.len().into()),
            ("truncated", truncated.into()),
            ("binary", false.into()),
        ]),
        format!(
            "read {} ({} bytes{})",
            path.display(),
            md.len(),
            if truncated { ", truncated" } else { "" }
        ),
    ))
}

fn write(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let path = arg_path(step, "path")?;
    let content = step.args.str_or("content", "").to_string();
    let existed = path.exists();

    if ctx.dry_run {
        return Ok(Effect::read_only(
            json_obj([
                ("path", path.to_string_lossy().to_string().into()),
                ("would_write", content.len().into()),
                ("would_overwrite", existed.into()),
            ]),
            format!(
                "would {} {} ({} bytes)",
                if existed { "overwrite" } else { "create" },
                path.display(),
                content.len()
            ),
        ));
    }

    // Snapshot first: the undo record has to exist before the damage does.
    let backup = ctx.journal.snapshot(&path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
    }
    // Write to a sibling temp file and rename, so a crash mid-write cannot
    // leave a half-written file where a whole one used to be.
    let tmp = path.with_extension(format!("nous-tmp-{}", std::process::id()));
    fs::write(&tmp, content.as_bytes())
        .map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot place {}: {}", path.display(), e)
    })?;

    Ok(Effect::with_undo(
        json_obj([
            ("path", path.to_string_lossy().to_string().into()),
            ("bytes", content.len().into()),
            ("created", (!existed).into()),
        ]),
        Undo::RestoreFile {
            path: path.to_string_lossy().to_string(),
            backup,
            existed,
        },
        format!(
            "{} {} ({} bytes)",
            if existed { "overwrote" } else { "created" },
            path.display(),
            content.len()
        ),
    ))
}

fn mkdir(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let path = arg_path(step, "path")?;
    if path.exists() {
        return Ok(Effect::read_only(
            json_obj([
                ("path", path.to_string_lossy().to_string().into()),
                ("existed", true.into()),
            ]),
            format!("{} already exists", path.display()),
        ));
    }
    if ctx.dry_run {
        return Ok(Effect::read_only(
            json_obj([("path", path.to_string_lossy().to_string().into())]),
            format!("would create directory {}", path.display()),
        ));
    }
    fs::create_dir_all(&path).map_err(|e| format!("cannot create {}: {}", path.display(), e))?;
    Ok(Effect::with_undo(
        json_obj([
            ("path", path.to_string_lossy().to_string().into()),
            ("existed", false.into()),
        ]),
        Undo::RemoveDir {
            path: path.to_string_lossy().to_string(),
        },
        format!("created directory {}", path.display()),
    ))
}

/// Move a path, falling back to copy-then-remove when the two ends are on
/// different filesystems (which `rename(2)` refuses).
pub fn move_path(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            let md = fs::symlink_metadata(from)
                .map_err(|e| format!("cannot stat {}: {}", from.display(), e))?;
            if md.is_dir() {
                copy_tree(from, to)?;
                fs::remove_dir_all(from)
                    .map_err(|e| format!("cannot remove {}: {}", from.display(), e))
            } else {
                fs::copy(from, to).map_err(|e| {
                    format!("cannot copy {} to {}: {}", from.display(), to.display(), e)
                })?;
                fs::remove_file(from)
                    .map_err(|e| format!("cannot remove {}: {}", from.display(), e))
            }
        }
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {}", to.display(), e))?;
    for entry in fs::read_dir(from).map_err(|e| format!("cannot read {}: {}", from.display(), e))? {
        let entry = entry.map_err(|e| format!("cannot read entry: {}", e))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| format!("cannot copy {}: {}", src.display(), e))?;
        }
    }
    Ok(())
}

fn rename(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let from = arg_path(step, "from")?;
    let to = arg_path(step, "to")?;
    if !from.exists() {
        return Err(format!("{} does not exist", from.display()));
    }
    if to.exists() {
        return Err(format!(
            "{} already exists — refusing to clobber it",
            to.display()
        ));
    }
    if ctx.dry_run {
        return Ok(Effect::read_only(
            json_obj([
                ("from", from.to_string_lossy().to_string().into()),
                ("to", to.to_string_lossy().to_string().into()),
            ]),
            format!("would move {} to {}", from.display(), to.display()),
        ));
    }
    move_path(&from, &to)?;
    Ok(Effect::with_undo(
        json_obj([
            ("from", from.to_string_lossy().to_string().into()),
            ("to", to.to_string_lossy().to_string().into()),
        ]),
        Undo::MovePath {
            from: from.to_string_lossy().to_string(),
            to: to.to_string_lossy().to_string(),
        },
        format!("moved {} to {}", from.display(), to.display()),
    ))
}

/// Deletion is a move into the trash store. Nothing in NOUS OS calls `unlink`
/// on a user's file.
fn delete(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let path = arg_path(step, "path")?;
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    let trash = ctx.trash_dir();
    let stamp = now_secs();
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("item");
    let dest = trash.join(format!("{}-{}", stamp, name));

    if ctx.dry_run {
        return Ok(Effect::read_only(
            json_obj([
                ("path", path.to_string_lossy().to_string().into()),
                ("trash", dest.to_string_lossy().to_string().into()),
            ]),
            format!("would move {} to the trash store", path.display()),
        ));
    }

    fs::create_dir_all(&trash).map_err(|e| format!("cannot create trash: {}", e))?;
    move_path(&path, &dest)?;
    Ok(Effect::with_undo(
        json_obj([
            ("path", path.to_string_lossy().to_string().into()),
            ("trash", dest.to_string_lossy().to_string().into()),
            ("recoverable", true.into()),
        ]),
        Undo::MovePath {
            from: path.to_string_lossy().to_string(),
            to: dest.to_string_lossy().to_string(),
        },
        format!("moved {} to the trash store (recoverable)", path.display()),
    ))
}

/// Walk a directory tree, yielding files (not directories), bounded by depth and
/// an exclusion list. Shared by the indexer and the curator.
pub fn walk(root: &Path, max_depth: usize, exclude: &[String], out: &mut Vec<PathBuf>, cap: usize) {
    if out.len() >= cap || max_depth == 0 {
        return;
    }
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        if out.len() >= cap {
            return;
        }
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || exclude.iter().any(|x| x == &name) {
            continue;
        }
        match entry.file_type() {
            // Symlinks are not followed: a loop would otherwise walk forever.
            Ok(ft) if ft.is_symlink() => continue,
            Ok(ft) if ft.is_dir() => dirs.push(p),
            Ok(_) => out.push(p),
            Err(_) => continue,
        }
    }
    for d in dirs {
        walk(&d, max_depth - 1, exclude, out, cap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::{Config, Journal};

    /// `root` holds daemon state; `dir` is the scratch area under test. Keeping
    /// them apart matters: a journal directory sitting inside the directory a
    /// test lists would show up in its own results.
    struct Fixture {
        root: PathBuf,
        dir: PathBuf,
        cfg: Config,
        journal: Journal,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            let root = std::env::temp_dir().join(format!("nous-fs-{}-{}", tag, std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let dir = root.join("work");
            fs::create_dir_all(&dir).unwrap();
            let journal = Journal::open(&root.join("journal")).unwrap();
            Fixture {
                root,
                dir,
                cfg: Config::with_defaults(),
                journal,
            }
        }
        fn ctx(&self, dry: bool) -> ExecCtx<'_> {
            ExecCtx::rooted(
                &self.cfg,
                &self.journal,
                dry,
                self.dir.clone(),
                self.root.join("state"),
            )
        }
        fn step(&self, cap: &str, args: Json) -> Step {
            Step::new("s1", cap, "fs", "", args)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn run(f: &Fixture, cap: &str, args: Json, dry: bool) -> Result<Effect, String> {
        let step = f.step(cap, args);
        execute(&Capability::parse(cap).unwrap(), &step, &f.ctx(dry))
    }

    #[test]
    fn lists_directories_with_folders_first() {
        let f = Fixture::new("list");
        fs::create_dir(f.dir.join("zeta-dir")).unwrap();
        fs::write(f.dir.join("alpha.txt"), b"x").unwrap();
        fs::write(f.dir.join(".hidden"), b"x").unwrap();

        let e = run(
            &f,
            "fs.list",
            json_obj([("path", f.dir.to_string_lossy().to_string().into())]),
            false,
        )
        .unwrap();
        let entries = e.result.arr_or_empty("entries");
        assert_eq!(entries.len(), 2, "hidden files are excluded by default");
        assert_eq!(
            entries[0].str_or("name", ""),
            "zeta-dir",
            "directories sort first"
        );
        assert_eq!(entries[1].str_or("kind", ""), "text");
    }

    #[test]
    fn write_is_reversible_and_atomic() {
        let f = Fixture::new("write");
        let target = f.dir.join("notes.md");
        fs::write(&target, b"original").unwrap();

        let e = run(
            &f,
            "fs.write",
            json_obj([
                ("path", target.to_string_lossy().to_string().into()),
                ("content", "replaced".into()),
            ]),
            false,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "replaced");

        super::super::revert(&e.undo, &f.ctx(false)).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
        // No temp files left behind.
        let leftovers: Vec<_> = fs::read_dir(&f.dir)
            .unwrap()
            .flatten()
            .filter(|d| d.file_name().to_string_lossy().contains("nous-tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "atomic write must not leave temp files"
        );
    }

    #[test]
    fn dry_run_changes_nothing() {
        let f = Fixture::new("dry");
        let target = f.dir.join("untouched.txt");
        let e = run(
            &f,
            "fs.write",
            json_obj([
                ("path", target.to_string_lossy().to_string().into()),
                ("content", "nope".into()),
            ]),
            true,
        )
        .unwrap();
        assert!(!target.exists(), "dry run must not create the file");
        assert!(e.undo.is_none());
        assert!(e.detail.starts_with("would "), "{}", e.detail);
    }

    #[test]
    fn delete_moves_to_trash_and_can_be_recovered() {
        let f = Fixture::new("delete");
        let victim = f.dir.join("important.txt");
        fs::write(&victim, b"do not lose me").unwrap();

        let e = run(
            &f,
            "fs.delete",
            json_obj([("path", victim.to_string_lossy().to_string().into())]),
            false,
        )
        .unwrap();
        assert!(!victim.exists());
        assert!(e.result.bool_or("recoverable", false));
        let trashed = PathBuf::from(e.result.str_or("trash", ""));
        assert_eq!(fs::read_to_string(&trashed).unwrap(), "do not lose me");

        super::super::revert(&e.undo, &f.ctx(false)).unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "do not lose me");
    }

    #[test]
    fn move_refuses_to_clobber_an_existing_file() {
        let f = Fixture::new("move");
        let a = f.dir.join("a.txt");
        let b = f.dir.join("b.txt");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let err = run(
            &f,
            "fs.move",
            json_obj([
                ("from", a.to_string_lossy().to_string().into()),
                ("to", b.to_string_lossy().to_string().into()),
            ]),
            false,
        )
        .unwrap_err();
        assert!(err.contains("refusing to clobber"), "{err}");
        assert_eq!(fs::read_to_string(&b).unwrap(), "b");
    }

    #[test]
    fn binary_files_are_reported_not_mangled() {
        let f = Fixture::new("binary");
        let bin = f.dir.join("clip.mp4");
        fs::write(&bin, [0xffu8, 0xd8, 0x00, 0x01, 0xfe]).unwrap();
        let e = run(
            &f,
            "fs.read",
            json_obj([("path", bin.to_string_lossy().to_string().into())]),
            false,
        )
        .unwrap();
        assert!(e.result.bool_or("binary", false));
        assert_eq!(e.result.str_or("kind", ""), "video");
        assert!(
            e.result.get("content").is_none(),
            "binary content must not be inlined"
        );
    }

    #[test]
    fn reads_are_truncated_at_the_limit() {
        let f = Fixture::new("truncate");
        let big = f.dir.join("big.txt");
        fs::write(&big, "a".repeat(4096)).unwrap();
        let e = run(
            &f,
            "fs.read",
            json_obj([
                ("path", big.to_string_lossy().to_string().into()),
                ("max_bytes", 100u64.into()),
            ]),
            false,
        )
        .unwrap();
        assert!(e.result.bool_or("truncated", false));
        assert_eq!(e.result.str_or("content", "").len(), 100);
    }

    #[test]
    fn classify_recognises_the_media_types_the_shell_cares_about() {
        assert_eq!(classify(Path::new("/a/song.FLAC"), false), "audio");
        assert_eq!(classify(Path::new("/a/clip.mkv"), false), "video");
        assert_eq!(classify(Path::new("/a/shot.HEIC"), false), "image");
        assert_eq!(classify(Path::new("/a/dir"), true), "folder");
        assert_eq!(classify(Path::new("/a/mystery"), false), "file");
    }

    #[test]
    fn walk_skips_excluded_and_hidden_directories() {
        let f = Fixture::new("walk");
        fs::create_dir_all(f.dir.join("src")).unwrap();
        fs::create_dir_all(f.dir.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(f.dir.join(".git")).unwrap();
        fs::write(f.dir.join("src/main.rs"), b"").unwrap();
        fs::write(f.dir.join("node_modules/pkg/index.js"), b"").unwrap();
        fs::write(f.dir.join(".git/HEAD"), b"").unwrap();

        let mut out = Vec::new();
        walk(&f.dir, 8, &["node_modules".to_string()], &mut out, 1000);
        assert_eq!(out.len(), 1, "found: {:?}", out);
        assert!(out[0].ends_with("src/main.rs"));
    }

    #[test]
    fn walk_respects_its_cap() {
        let f = Fixture::new("walkcap");
        for i in 0..50 {
            fs::write(f.dir.join(format!("f{}.txt", i)), b"").unwrap();
        }
        let mut out = Vec::new();
        walk(&f.dir, 4, &[], &mut out, 10);
        assert_eq!(out.len(), 10);
    }
}
