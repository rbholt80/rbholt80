//! `nous --check` — what works, what does not, and why.
//!
//! Written because the window is where things silently do not happen. A
//! folder with no marks on it looks exactly like a tidy folder; a player
//! showing nothing looks exactly like a player with nothing to play. When
//! something is wrong the interface's job is to carry on working, which means
//! the interface is the wrong place to find out what is wrong.
//!
//! So this asks every question the window asks, one at a time, in a terminal,
//! and prints the answer. It needs no display, so it works over ssh and on a
//! machine where the window will not open at all — which is exactly when
//! somebody needs it.

use crate::link::{report, Link};
use nous_core::json::{json_obj, Json};
use std::path::PathBuf;

/// Exit code: 0 when everything the window needs is there.
pub fn run() -> i32 {
    let mut trouble = 0;
    println!("nous {}", env!("CARGO_PKG_VERSION"));

    // 1. A display. Not fatal to this check, and fatal to the window.
    match std::env::var("DISPLAY") {
        Ok(d) if !d.is_empty() => say("display", &d, true),
        _ => {
            say("display", "not set — the window cannot open", false);
            trouble += 1;
        }
    }

    // 2. The daemon. Everything that changes anything goes through it.
    let sock = nous_core::ipc::socket_path();
    let mut link = Link::new();
    match link.ping() {
        true => say("daemon", &sock.to_string_lossy(), true),
        false => {
            let why = link
                .trouble
                .clone()
                .unwrap_or_else(|| "no answer".to_string());
            say(
                "daemon",
                &format!("{why} — looked at {}", sock.display()),
                false,
            );
            println!();
            println!("  Start it with:  nousd &");
            println!("  Without it you can look at your files but not change them.");
            return 1;
        }
    }

    // 3. The things the window asks for by name, each reported on its own so
    //    one being absent does not read as all of them being.
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let folder = {
        let d = home.join("Downloads");
        if d.is_dir() {
            d
        } else {
            home.clone()
        }
    };

    trouble += probe(
        &mut link,
        "curator",
        "curate.scan",
        json_obj([(
            "roots",
            Json::Arr(vec![folder.to_string_lossy().to_string().into()]),
        )]),
        |v| {
            let n = v.arr_or_empty("findings").len();
            let space = v.str_or("reclaimable", "");
            if n == 0 {
                format!("nothing to tidy in {}", folder.display())
            } else if space.is_empty() {
                format!("{n} findings in {}", folder.display())
            } else {
                format!("{n} findings in {} · {space} reclaimable", folder.display())
            }
        },
    );

    trouble += probe(&mut link, "search", "fs.search", json_obj([]), |v| {
        let n = v.f64_or("indexed", 0.0) as u64;
        if n == 0 {
            "no index yet — run `nousctl index` to build one".to_string()
        } else {
            format!("{n} files indexed")
        }
    });

    trouble += probe(&mut link, "playback", "media.state", Json::obj(), |v| {
        if v.bool_or("playing", false) {
            format!("playing {}", v.str_or("path", "something"))
        } else {
            "nothing playing".to_string()
        }
    });

    // 4. The outside programs the media side shells out to.
    for (what, prog) in [
        ("player", "mpv"),
        ("encoder", "ffmpeg"),
        ("probe", "ffprobe"),
    ] {
        let found = which(prog);
        if !found {
            trouble += 1;
        }
        say(
            what,
            &if found {
                prog.to_string()
            } else {
                format!("{prog} not installed — music and video will not play")
            },
            found,
        );
    }

    println!();
    if trouble == 0 {
        println!("Everything the window needs is here.");
    } else {
        println!("{trouble} thing(s) above will not work. The rest will.");
    }
    // The window is usable without any of the optional pieces, so this reports
    // trouble without calling it failure — a non-zero exit here would make a
    // machine with no mpv look broken.
    0
}

/// Ask for one thing and describe what came back.
///
/// Returns 1 when it did not work, so the caller can count.
fn probe(
    link: &mut Link,
    label: &str,
    cap: &str,
    args: Json,
    describe: impl Fn(&Json) -> String,
) -> i32 {
    match link.invoke(cap, args, cap) {
        Ok(reply) => {
            let v = report::value(&reply);
            say(label, &describe(&v), true);
            0
        }
        Err(e) => {
            say(label, &format!("{cap}: {e}"), false);
            1
        }
    }
}

fn say(label: &str, detail: &str, good: bool) {
    // Two columns and a mark, so a wall of these can be read down the left
    // edge without reading any of it.
    println!(
        "  {} {:<9} {}",
        if good { "ok " } else { "-- " },
        label,
        detail
    );
}

fn which(prog: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':')
        .any(|d| !d.is_empty() && std::path::Path::new(d).join(prog).is_file())
}
