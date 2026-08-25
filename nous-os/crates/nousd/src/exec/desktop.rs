//! Desktop executor: acting on the session you are already running.
//!
//! This is what lets NOUS live *on top of* an existing desktop — Cinnamon on
//! Linux Mint, but equally GNOME, KDE or XFCE — instead of replacing it. Your
//! window manager, your file manager and your applications stay exactly as they
//! are; NOUS gains the ability to see and act on them.
//!
//! Two things govern how this module is written.
//!
//! **It never assumes a tool is present.** A desktop is an assortment of
//! programs that may or may not be installed, and a missing one must produce a
//! sentence telling you what to install, not an opaque failure.
//!
//! **Looking is not free here.** Reading the clipboard and capturing the screen
//! are classed `elevated`, not `read`, because whatever is there right now may
//! be a password or someone else's message — and unlike a file, you did not
//! choose to put it in front of the system.

use super::{Effect, ExecCtx};
use super::sysops::{have, run};
use nous_core::cap::Capability;
use nous_core::journal::Undo;
use nous_core::json::{json_obj, Json};
use nous_core::Step;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

pub fn execute(cap: &Capability, step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    match cap.action.as_str() {
        "apps" => apps(step),
        "windows" => windows(),
        "session_info" => Ok(Effect::read_only(session_info(), "described the desktop session")),
        "notify" => notify(step, ctx),
        "copy" => copy(step, ctx),
        "clipboard" => clipboard(ctx),
        "focus" => focus(step, ctx),
        "open" => open(step, ctx),
        "launch" => launch(step, ctx),
        "close" => close_window(step, ctx),
        "screenshot" => screenshot(step, ctx),
        "setting" => setting(step, ctx),
        "session" => session(step, ctx),
        other => Err(format!("desktop executor cannot '{}'", other)),
    }
}

// ----------------------------------------------------------------- session

/// X11 or Wayland, and which desktop is running.
///
/// This matters more than it looks: half the tooling below only works on one of
/// them, and telling you *why* something is unavailable is more useful than
/// telling you that it is.
pub fn session_kind() -> &'static str {
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        return "wayland";
    }
    match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => "wayland",
        Ok("x11") => "x11",
        _ => {
            if std::env::var("DISPLAY").is_ok() {
                "x11"
            } else {
                "none"
            }
        }
    }
}

pub fn desktop_name() -> String {
    for var in ["XDG_CURRENT_DESKTOP", "DESKTOP_SESSION", "XDG_SESSION_DESKTOP"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    "unknown".to_string()
}

pub fn session_info() -> Json {
    let kind = session_kind();
    let tools: Vec<Json> = TOOLS
        .iter()
        .map(|(name, purpose, package)| {
            json_obj([
                ("tool", (*name).into()),
                ("purpose", (*purpose).into()),
                ("package", (*package).into()),
                ("present", have(name).into()),
            ])
        })
        .collect();
    let missing: Vec<&str> =
        TOOLS.iter().filter(|(n, _, _)| !have(n)).map(|(_, _, p)| *p).collect();

    json_obj([
        ("session", kind.into()),
        ("desktop", desktop_name().into()),
        ("graphical", (kind != "none").into()),
        ("tools", Json::Arr(tools)),
        (
            "install_hint",
            if missing.is_empty() {
                Json::Null
            } else {
                let mut unique: Vec<&str> = missing;
                unique.sort_unstable();
                unique.dedup();
                Json::Str(format!("sudo apt install {}", unique.join(" ")))
            },
        ),
    ])
}

/// The programs this module drives, what each is for, and the Debian/Mint
/// package that provides it.
const TOOLS: &[(&str, &str, &str)] = &[
    ("wmctrl", "list, focus and close windows", "wmctrl"),
    ("xdotool", "window details and keyboard input", "xdotool"),
    ("xclip", "read and write the clipboard", "xclip"),
    ("notify-send", "desktop notifications", "libnotify-bin"),
    ("xdg-open", "open a file in its usual application", "xdg-utils"),
    ("gsettings", "read and change desktop settings", "libglib2.0-bin"),
];

/// Find the first available program from a list, or explain what to install.
fn first_tool(candidates: &[&str], purpose: &str) -> Result<String, String> {
    for c in candidates {
        if have(c) {
            return Ok((*c).to_string());
        }
    }
    let packages: Vec<&str> = candidates
        .iter()
        .filter_map(|c| TOOLS.iter().find(|(n, _, _)| n == c).map(|(_, _, p)| *p))
        .collect();
    let hint = if packages.is_empty() {
        candidates.join(" or ")
    } else {
        format!("sudo apt install {}", packages.join(" "))
    };
    Err(format!("cannot {} — no tool for it is installed. Try: {}", purpose, hint))
}

// ------------------------------------------------------------------- apps

/// Parse a `.desktop` file into the fields a launcher needs.
///
/// Only the `[Desktop Entry]` group is read; a `.desktop` file may contain
/// several groups, and actions defined in the others are not the application.
pub fn parse_desktop_entry(text: &str) -> Option<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim();
            // Localised keys look like `Name[fr]`. Keep the unlocalised one.
            if key.contains('[') {
                continue;
            }
            fields.insert(key.to_string(), v.trim().to_string());
        }
    }
    if fields.is_empty() {
        None
    } else {
        Some(fields)
    }
}

/// Strip the field codes the desktop entry spec allows in `Exec`.
///
/// `firefox %u` must become `firefox`, or the launched process receives a
/// literal `%u` as an argument.
pub fn clean_exec(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.peek() {
                // %% is a literal percent sign.
                Some('%') => {
                    chars.next();
                    out.push('%');
                }
                Some(code) if "fFuUdDnNickvm".contains(*code) => {
                    chars.next();
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
        PathBuf::from("/var/lib/snapd/desktop/applications"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/applications"));
        dirs.push(PathBuf::from(&home).join(".local/share/flatpak/exports/share/applications"));
    }
    dirs
}

/// Everything installed that a person could launch.
pub fn installed_apps(dirs: &[PathBuf]) -> Vec<Json> {
    let mut seen: BTreeMap<String, Json> = BTreeMap::new();
    for dir in dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("desktop") {
                continue;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let fields = match parse_desktop_entry(&text) {
                Some(f) => f,
                None => continue,
            };

            let truthy = |k: &str| {
                fields.get(k).map(|v| v.eq_ignore_ascii_case("true")).unwrap_or(false)
            };
            if truthy("NoDisplay") || truthy("Hidden") {
                continue;
            }
            if fields.get("Type").map(|t| t != "Application").unwrap_or(false) {
                continue;
            }
            let name = match fields.get("Name") {
                Some(n) if !n.is_empty() => n.clone(),
                _ => continue,
            };
            let exec = fields.get("Exec").map(|e| clean_exec(e)).unwrap_or_default();
            if exec.is_empty() {
                continue;
            }
            let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();

            // A user's own ~/.local override should win over the system copy,
            // and it is scanned last, so overwrite rather than skip.
            seen.insert(
                id.clone(),
                json_obj([
                    ("id", id.into()),
                    ("name", name.into()),
                    ("exec", exec.into()),
                    ("comment", fields.get("Comment").cloned().unwrap_or_default().into()),
                    ("icon", fields.get("Icon").cloned().unwrap_or_default().into()),
                    ("categories", fields.get("Categories").cloned().unwrap_or_default().into()),
                    ("terminal", truthy("Terminal").into()),
                    ("path", path.to_string_lossy().to_string().into()),
                ]),
            );
        }
    }

    let mut out: Vec<Json> = seen.into_values().collect();
    out.sort_by(|a, b| a.str_or("name", "").to_lowercase().cmp(&b.str_or("name", "").to_lowercase()));
    out
}

fn apps(step: &Step) -> Result<Effect, String> {
    let query = step.args.str_or("query", "").to_ascii_lowercase();
    let mut list = installed_apps(&application_dirs());
    if !query.is_empty() {
        list.retain(|a| {
            let hay = format!("{} {} {}", a.str_or("name", ""), a.str_or("comment", ""), a.str_or("categories", ""))
                .to_ascii_lowercase();
            query.split_whitespace().all(|t| hay.contains(t))
        });
    }
    let n = list.len();
    Ok(Effect::read_only(
        json_obj([("apps", Json::Arr(list)), ("count", n.into())]),
        format!("found {} application{}", n, if n == 1 { "" } else { "s" }),
    ))
}

// ---------------------------------------------------------------- windows

/// Parse one line of `wmctrl -lpx` output.
///
/// The format is id, desktop, pid, WM_CLASS, host, then the title — which may
/// itself contain spaces, so it is everything after the fifth field.
pub fn parse_wmctrl_line(line: &str) -> Option<Json> {
    // Walk the five fixed fields explicitly: `splitn` cannot be trusted here
    // because runs of spaces between columns vary with the window id width.
    let mut fields = Vec::new();
    let mut rest = line.trim_start();
    for _ in 0..5 {
        let idx = rest.find(char::is_whitespace)?;
        fields.push(&rest[..idx]);
        rest = rest[idx..].trim_start();
    }

    let class = fields[3];
    // WM_CLASS arrives as `instance.Class`; the second half is the useful one.
    let app = class.rsplit('.').next().unwrap_or(class);
    Some(json_obj([
        ("id", fields[0].into()),
        ("workspace", fields[1].parse::<f64>().unwrap_or(-1.0).into()),
        ("pid", fields[2].parse::<f64>().unwrap_or(0.0).into()),
        ("class", class.into()),
        ("app", app.into()),
        ("title", rest.trim().into()),
    ]))
}

fn windows() -> Result<Effect, String> {
    if session_kind() == "none" {
        return Err("there is no graphical session to look at".to_string());
    }
    let tool = first_tool(&["wmctrl"], "list windows")?;
    let out = run(&tool, &["-lpx"], Duration::from_secs(10))?;
    out.require("wmctrl")?;

    let list: Vec<Json> = out.stdout.lines().filter_map(parse_wmctrl_line).collect();
    let n = list.len();
    Ok(Effect::read_only(
        json_obj([("windows", Json::Arr(list)), ("count", n.into())]),
        format!("{} window{} open", n, if n == 1 { "" } else { "s" }),
    ))
}

/// Find the window whose title or application best matches a phrase.
pub fn match_window(windows: &[Json], phrase: &str) -> Option<Json> {
    let q = phrase.trim().to_ascii_lowercase();
    if q.is_empty() {
        return None;
    }
    // An application-name match beats a title match: "close firefox" means the
    // browser, not the page that happens to mention Firefox.
    windows
        .iter()
        .find(|w| w.str_or("app", "").to_ascii_lowercase() == q)
        .or_else(|| windows.iter().find(|w| w.str_or("app", "").to_ascii_lowercase().contains(&q)))
        .or_else(|| windows.iter().find(|w| w.str_or("title", "").to_ascii_lowercase().contains(&q)))
        .cloned()
}

fn window_by_phrase(step: &Step) -> Result<Json, String> {
    if let Some(id) = step.args.get("id").and_then(|v| v.as_str()) {
        return Ok(json_obj([("id", id.into()), ("title", id.into()), ("app", "".into())]));
    }
    let phrase = step.args.str_or("window", step.args.str_or("name", ""));
    if phrase.is_empty() {
        return Err("which window? name the application or part of its title".to_string());
    }
    let tool = first_tool(&["wmctrl"], "find windows")?;
    let out = run(&tool, &["-lpx"], Duration::from_secs(10))?;
    out.require("wmctrl")?;
    let list: Vec<Json> = out.stdout.lines().filter_map(parse_wmctrl_line).collect();
    match_window(&list, phrase)
        .ok_or_else(|| format!("no open window matches '{}'", phrase))
}

fn focus(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let w = window_by_phrase(step)?;
    let id = w.str_or("id", "").to_string();
    if ctx.dry_run {
        return Ok(Effect::read_only(w.clone(), format!("would focus {}", w.str_or("title", &id))));
    }
    let tool = first_tool(&["wmctrl"], "focus a window")?;
    run(&tool, &["-ia", &id], Duration::from_secs(10))?.require("wmctrl")?;
    Ok(Effect::read_only(w.clone(), format!("focused {}", w.str_or("title", &id))))
}

fn close_window(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let w = window_by_phrase(step)?;
    let id = w.str_or("id", "").to_string();
    let title = w.str_or("title", &id).to_string();
    if ctx.dry_run {
        return Ok(Effect::read_only(w.clone(), format!("would close {}", title)));
    }
    let tool = first_tool(&["wmctrl"], "close a window")?;
    // `-c` asks the window to close, so the application still gets to prompt
    // about unsaved work. Nothing here kills a process.
    run(&tool, &["-ic", &id], Duration::from_secs(10))?.require("wmctrl")?;
    Ok(Effect::with_undo(
        w,
        Undo::Manual { note: format!("reopen {}", title) },
        format!("asked {} to close", title),
    ))
}

// -------------------------------------------------------------- clipboard

fn clipboard(ctx: &ExecCtx) -> Result<Effect, String> {
    if ctx.dry_run {
        return Ok(Effect::read_only(Json::obj(), "would read the clipboard"));
    }
    let text = if session_kind() == "wayland" && have("wl-paste") {
        run("wl-paste", &["--no-newline"], Duration::from_secs(5))?.stdout
    } else {
        let tool = first_tool(&["xclip", "xsel"], "read the clipboard")?;
        let args: Vec<&str> = if tool == "xclip" {
            vec!["-selection", "clipboard", "-o"]
        } else {
            vec!["--clipboard", "--output"]
        };
        run(&tool, &args, Duration::from_secs(5))?.stdout
    };
    let chars = text.chars().count();
    Ok(Effect::read_only(
        json_obj([("text", text.into()), ("length", chars.into())]),
        format!("read {} characters from the clipboard", chars),
    ))
}

fn copy(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let text = step.args.str_or("text", "").to_string();
    if text.is_empty() {
        return Err("nothing to copy".to_string());
    }
    if ctx.dry_run {
        return Ok(Effect::read_only(Json::obj(), "would put text on the clipboard"));
    }

    use std::io::Write;
    use std::process::{Command, Stdio};
    let (program, args): (String, Vec<&str>) = if session_kind() == "wayland" && have("wl-copy") {
        ("wl-copy".to_string(), vec![])
    } else {
        let tool = first_tool(&["xclip", "xsel"], "write to the clipboard")?;
        if tool == "xclip" {
            (tool, vec!["-selection", "clipboard"])
        } else {
            (tool, vec!["--clipboard", "--input"])
        }
    };

    let mut child = Command::new(&program)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {}", program, e))?;
    child
        .stdin
        .as_mut()
        .ok_or("cannot write to the clipboard tool")?
        .write_all(text.as_bytes())
        .map_err(|e| format!("cannot write to the clipboard: {}", e))?;
    child.wait().map_err(|e| format!("clipboard tool failed: {}", e))?;

    Ok(Effect::with_undo(
        json_obj([("length", text.chars().count().into())]),
        Undo::Manual { note: "the previous clipboard contents were replaced".to_string() },
        format!("copied {} characters", text.chars().count()),
    ))
}

// ------------------------------------------------------- notify and launch

fn notify(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let message = step.args.str_or("message", step.args.str_or("text", "")).to_string();
    if message.is_empty() {
        return Err("nothing to say".to_string());
    }
    let title = step.args.str_or("title", "NOUS").to_string();
    if ctx.dry_run {
        return Ok(Effect::read_only(Json::obj(), format!("would notify: {}", message)));
    }
    let tool = first_tool(&["notify-send"], "show a notification")?;
    let urgency = step.args.str_or("urgency", "normal").to_string();
    run(&tool, &["-a", "NOUS", "-u", &urgency, &title, &message], Duration::from_secs(10))?
        .require("notify-send")?;
    Ok(Effect::read_only(
        json_obj([("title", title.into()), ("message", message.clone().into())]),
        format!("notified: {}", message),
    ))
}

fn open(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let target = step
        .args
        .get("path")
        .or_else(|| step.args.get("url"))
        .and_then(|v| v.as_str())
        .ok_or("what should be opened?")?
        .to_string();
    let resolved = if target.starts_with("http://") || target.starts_with("https://") {
        target.clone()
    } else {
        let p = nous_core::config::expand_tilde(&target);
        if !p.exists() {
            return Err(format!("{} does not exist", p.display()));
        }
        p.to_string_lossy().to_string()
    };
    if ctx.dry_run {
        return Ok(Effect::read_only(Json::obj(), format!("would open {}", resolved)));
    }
    let tool = first_tool(&["xdg-open", "gio"], "open a file")?;
    let args: Vec<&str> =
        if tool == "gio" { vec!["open", &resolved] } else { vec![&resolved] };
    // Detached: the opened application outlives this request.
    std::process::Command::new(&tool)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot open {}: {}", resolved, e))?;
    Ok(Effect::read_only(
        json_obj([("opened", resolved.clone().into())]),
        format!("opened {}", resolved),
    ))
}

fn launch(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let wanted = step.args.str_or("name", step.args.str_or("app", "")).to_string();
    if wanted.is_empty() {
        return Err("which application?".to_string());
    }
    let apps = installed_apps(&application_dirs());
    let lower = wanted.to_ascii_lowercase();
    let app = apps
        .iter()
        .find(|a| a.str_or("name", "").to_ascii_lowercase() == lower)
        .or_else(|| apps.iter().find(|a| a.str_or("id", "").to_ascii_lowercase() == lower))
        .or_else(|| apps.iter().find(|a| a.str_or("name", "").to_ascii_lowercase().contains(&lower)))
        .ok_or_else(|| format!("no installed application matches '{}'", wanted))?;

    let exec = app.str_or("exec", "").to_string();
    let name = app.str_or("name", "").to_string();
    if ctx.dry_run {
        return Ok(Effect::read_only(app.clone(), format!("would launch {} ({})", name, exec)));
    }

    let mut parts = exec.split_whitespace();
    let program = parts.next().ok_or("this application has no command to run")?;
    let args: Vec<&str> = parts.collect();
    let child = std::process::Command::new(program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot launch {}: {}", name, e))?;

    Ok(Effect::with_undo(
        json_obj([("name", name.clone().into()), ("pid", (child.id() as u64).into())]),
        Undo::Manual { note: format!("close {}", name) },
        format!("launched {}", name),
    ))
}

// ------------------------------------------------------ screen and session

fn screenshot(_step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    if ctx.dry_run {
        return Ok(Effect::read_only(Json::obj(), "would capture the screen"));
    }
    let dir = ctx.state.join("screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    let out = dir.join(format!("{}.png", nous_core::journal::now_secs()));
    let path = out.to_string_lossy().to_string();

    // Tried in order of how likely each is to already be installed on a Mint
    // desktop, then how well it behaves without a prompt.
    let attempts: Vec<(&str, Vec<&str>)> = vec![
        ("gnome-screenshot", vec!["-f", &path]),
        ("scrot", vec![&path]),
        ("import", vec!["-window", "root", &path]),
        ("spectacle", vec!["-b", "-n", "-o", &path]),
        ("grim", vec![&path]),
    ];
    let mut tried = Vec::new();
    for (tool, args) in attempts {
        if !have(tool) {
            continue;
        }
        tried.push(tool);
        let r = run(tool, &args, Duration::from_secs(30))?;
        if r.ok() && out.exists() {
            let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            return Ok(Effect::with_undo(
                json_obj([("path", path.clone().into()), ("bytes", size.into()), ("tool", tool.into())]),
                Undo::RestoreFile { path: path.clone(), backup: None, existed: false },
                format!("captured the screen to {}", path),
            ));
        }
    }
    Err(if tried.is_empty() {
        "no screenshot tool is installed. Try: sudo apt install gnome-screenshot".to_string()
    } else {
        format!("the screenshot tools available ({}) all failed", tried.join(", "))
    })
}

fn setting(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let schema = step.args.str_or("schema", "").to_string();
    let key = step.args.str_or("key", "").to_string();
    if schema.is_empty() || key.is_empty() {
        return Err("a setting needs a schema and a key".to_string());
    }
    let tool = first_tool(&["gsettings"], "change a desktop setting")?;

    let previous = run(&tool, &["get", &schema, &key], Duration::from_secs(10))
        .ok()
        .map(|r| r.stdout.trim().to_string())
        .unwrap_or_default();

    let value = match step.args.get("value").and_then(|v| v.as_str()) {
        Some(v) => v.to_string(),
        // No value means read it.
        None => {
            return Ok(Effect::read_only(
                json_obj([("schema", schema.clone().into()), ("key", key.clone().into()), ("value", previous.clone().into())]),
                format!("{} {} is {}", schema, key, previous),
            ))
        }
    };

    if ctx.dry_run {
        return Ok(Effect::read_only(
            Json::obj(),
            format!("would set {} {} to {} (currently {})", schema, key, value, previous),
        ));
    }
    run(&tool, &["set", &schema, &key, &value], Duration::from_secs(10))?.require("gsettings")?;
    Ok(Effect::with_undo(
        json_obj([("schema", schema.clone().into()), ("key", key.clone().into()), ("value", value.clone().into())]),
        Undo::Manual { note: format!("gsettings set {} {} {}", schema, key, previous) },
        format!("set {} {} to {}", schema, key, value),
    ))
}

fn session(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let action = step.args.str_or("action", "lock").to_string();
    if ctx.dry_run {
        return Ok(Effect::read_only(Json::obj(), format!("would {} the session", action)));
    }
    let attempts: Vec<(&str, Vec<&str>)> = match action.as_str() {
        "lock" => vec![
            ("cinnamon-screensaver-command", vec!["--lock"]),
            ("xdg-screensaver", vec!["lock"]),
            ("loginctl", vec!["lock-session"]),
        ],
        "logout" => vec![
            ("cinnamon-session-quit", vec!["--logout", "--no-prompt"]),
            ("loginctl", vec!["terminate-session", "self"]),
        ],
        other => return Err(format!("unknown session action '{}'", other)),
    };
    for (tool, args) in attempts {
        if have(tool) && run(tool, &args, Duration::from_secs(15))?.ok() {
            return Ok(Effect::with_undo(
                json_obj([("action", action.clone().into())]),
                Undo::Manual { note: "log back in".to_string() },
                format!("{} requested", action),
            ));
        }
    }
    Err(format!("nothing on this system could {} the session", action))
}

/// The paths the file manager's context menu passes in, as a shared helper.
pub fn selected_paths(step: &Step) -> Vec<PathBuf> {
    step.args
        .str_list("paths")
        .iter()
        .map(|p| nous_core::config::expand_tilde(p))
        .filter(|p: &PathBuf| p.exists())
        .collect()
}

/// Describe what the user is looking at, for the resolver's benefit.
pub fn focus_context() -> Json {
    let mut ctx = json_obj([("session", session_kind().into()), ("desktop", desktop_name().into())]);
    if session_kind() == "x11" && have("xdotool") {
        if let Ok(r) = run("xdotool", &["getactivewindow", "getwindowname"], Duration::from_secs(3)) {
            if r.ok() {
                ctx.set("focused_window", Json::Str(r.stdout.trim().to_string()));
            }
        }
    }
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_desktop_entry() {
        let text = "\
[Desktop Entry]
Version=1.0
Name=Firefox Web Browser
Name[fr]=Navigateur Web Firefox
Comment=Browse the World Wide Web
Exec=firefox %u
Icon=firefox
Terminal=false
Type=Application
Categories=Network;WebBrowser;

[Desktop Action new-window]
Name=Open a New Window
Exec=firefox --new-window
";
        let f = parse_desktop_entry(text).unwrap();
        assert_eq!(f.get("Name").unwrap(), "Firefox Web Browser", "the localised name must not win");
        assert_eq!(f.get("Exec").unwrap(), "firefox %u");
        // Fields from the action group must not leak into the application.
        assert!(!f.get("Exec").unwrap().contains("--new-window"));
    }

    #[test]
    fn strips_desktop_field_codes_from_exec() {
        assert_eq!(clean_exec("firefox %u"), "firefox");
        assert_eq!(clean_exec("gimp-2.10 %U"), "gimp-2.10");
        assert_eq!(clean_exec("env FOO=1 code --unity-launch %F"), "env FOO=1 code --unity-launch");
        // %% is a literal percent, not a field code.
        assert_eq!(clean_exec("thing --pct 50%% %f"), "thing --pct 50%");
        assert_eq!(clean_exec("plain-command"), "plain-command");
    }

    #[test]
    fn enumerates_real_applications_from_disk() {
        // The container has a handful of genuine .desktop files.
        let apps = installed_apps(&[PathBuf::from("/usr/share/applications")]);
        assert!(!apps.is_empty(), "should find the system's installed applications");
        for a in &apps {
            assert!(!a.str_or("name", "").is_empty());
            assert!(!a.str_or("exec", "").is_empty());
            assert!(!a.str_or("exec", "").contains('%'), "field codes must be stripped");
        }
    }

    #[test]
    fn hidden_and_non_application_entries_are_skipped() {
        let dir = std::env::temp_dir().join(format!("nous-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("shown.desktop"),
            "[Desktop Entry]\nType=Application\nName=Shown\nExec=shown\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("hidden.desktop"),
            "[Desktop Entry]\nType=Application\nName=Hidden\nExec=hidden\nNoDisplay=true\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("link.desktop"),
            "[Desktop Entry]\nType=Link\nName=A Link\nURL=http://example.com\nExec=x\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("noexec.desktop"),
            "[Desktop Entry]\nType=Application\nName=No Command\n",
        )
        .unwrap();

        let apps = installed_apps(&[dir.clone()]);
        let names: Vec<String> = apps.iter().map(|a| a.str_or("name", "").to_string()).collect();
        assert_eq!(names, ["Shown"], "got {:?}", names);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_user_override_beats_the_system_copy() {
        let base = std::env::temp_dir().join(format!("nous-override-{}", std::process::id()));
        let (sys, user) = (base.join("sys"), base.join("user"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(sys.join("editor.desktop"), "[Desktop Entry]\nType=Application\nName=Editor\nExec=old-editor\n").unwrap();
        std::fs::write(user.join("editor.desktop"), "[Desktop Entry]\nType=Application\nName=Editor\nExec=new-editor\n").unwrap();

        let apps = installed_apps(&[sys, user]);
        assert_eq!(apps.len(), 1, "the same id must not appear twice");
        assert_eq!(apps[0].str_or("exec", ""), "new-editor");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn parses_a_wmctrl_window_line() {
        let line = "0x03c00007  1 12345  Navigator.Firefox  mintbox  Bug 42 - Mozilla Firefox";
        let w = parse_wmctrl_line(line).unwrap();
        assert_eq!(w.str_or("id", ""), "0x03c00007");
        assert_eq!(w.f64_or("workspace", -9.0), 1.0);
        assert_eq!(w.f64_or("pid", 0.0), 12345.0);
        assert_eq!(w.str_or("app", ""), "Firefox");
        assert_eq!(w.str_or("title", ""), "Bug 42 - Mozilla Firefox", "titles contain spaces");
    }

    #[test]
    fn malformed_window_lines_are_skipped_not_fatal() {
        assert!(parse_wmctrl_line("").is_none());
        assert!(parse_wmctrl_line("0x01 broken").is_none());
    }

    #[test]
    fn window_matching_prefers_the_application_over_the_title() {
        let windows = vec![
            parse_wmctrl_line("0x01 0 1 gedit.Gedit host Notes about firefox").unwrap(),
            parse_wmctrl_line("0x02 0 2 Navigator.Firefox host Mozilla Firefox").unwrap(),
        ];
        let hit = match_window(&windows, "firefox").unwrap();
        assert_eq!(hit.str_or("id", ""), "0x02", "'close firefox' means the browser");

        assert_eq!(match_window(&windows, "notes").unwrap().str_or("id", ""), "0x01");
        assert!(match_window(&windows, "inkscape").is_none());
        assert!(match_window(&windows, "").is_none());
    }

    #[test]
    fn a_missing_tool_says_what_to_install() {
        let err = first_tool(&["definitely-not-installed-xyz"], "do the thing").unwrap_err();
        assert!(err.contains("cannot do the thing"), "{err}");
    }

    #[test]
    fn missing_clipboard_tools_name_their_package() {
        let err = first_tool(&["xclip"], "read the clipboard").unwrap_err();
        // xclip is genuinely absent in this container.
        assert!(err.contains("apt install xclip"), "{err}");
    }

    #[test]
    fn session_info_reports_what_is_missing() {
        let info = session_info();
        assert!(info.get("tools").is_some());
        assert!(!info.str_or("session", "").is_empty());
        // Headless container: it should say so rather than pretending.
        let hint = info.get("install_hint").and_then(|v| v.as_str()).unwrap_or("");
        assert!(hint.starts_with("sudo apt install"), "{hint}");
    }

    #[test]
    fn desktop_actions_refuse_cleanly_without_a_session() {
        std::env::remove_var("DISPLAY");
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("XDG_SESSION_TYPE");
        assert_eq!(session_kind(), "none");
        let err = windows().unwrap_err();
        assert!(err.contains("no graphical session"), "{err}");
    }

    #[test]
    fn selected_paths_drop_things_that_are_gone() {
        let dir = std::env::temp_dir().join(format!("nous-sel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("here.txt");
        std::fs::write(&real, b"x").unwrap();

        let step = Step::new(
            "s",
            "desk.open",
            "desk",
            "",
            json_obj([(
                "paths",
                Json::Arr(vec![
                    Json::Str(real.to_string_lossy().to_string()),
                    Json::Str(dir.join("gone.txt").to_string_lossy().to_string()),
                ]),
            )]),
        );
        assert_eq!(selected_paths(&step).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
