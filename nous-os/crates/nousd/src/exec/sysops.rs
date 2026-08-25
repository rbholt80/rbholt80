//! System executor: machine state, processes, services, packages, shell.
//!
//! Where the information is in `/proc` or `/sys`, it is read directly rather
//! than by shelling out — parsing `ps` output is both slower and less reliable
//! than reading the files `ps` itself reads.

use super::{Effect, ExecCtx};
use nous_core::cap::Capability;
use nous_core::journal::Undo;
use nous_core::json::{json_obj, Json};
use nous_core::Step;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn execute(cap: &Capability, step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    match (cap.domain.as_str(), cap.action.as_str()) {
        ("sys", "info") => Ok(Effect::read_only(sys_info(), "described the machine")),
        ("sys", "metrics") => Ok(Effect::read_only(sys_metrics(), "sampled machine metrics")),
        ("sys", "power") => power(step, ctx),
        ("proc", "list") => proc_list(step),
        ("proc", "signal") => proc_signal(step, ctx),
        ("svc", "status") => svc_status(step, ctx),
        ("svc", "start") | ("svc", "stop") | ("svc", "restart") => svc_change(cap, step, ctx),
        ("pkg", "query") => pkg_query(step, ctx),
        ("pkg", "install") | ("pkg", "remove") => pkg_change(cap, step, ctx),
        ("net", "status") => Ok(Effect::read_only(net_status(ctx), "sampled network state")),
        ("shell", "exec") => shell_exec(step, ctx),
        (d, a) => Err(format!("system executor cannot '{}.{}'", d, a)),
    }
}

// ------------------------------------------------------------ process helpers

pub struct CmdOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl CmdOutput {
    pub fn ok(&self) -> bool {
        self.status == 0 && !self.timed_out
    }
    /// stdout if the command succeeded, otherwise a readable failure.
    pub fn require(&self, what: &str) -> Result<&str, String> {
        if self.timed_out {
            return Err(format!("{} timed out", what));
        }
        if self.status != 0 {
            let msg = if self.stderr.trim().is_empty() { &self.stdout } else { &self.stderr };
            return Err(format!("{} failed ({}): {}", what, self.status, msg.trim()));
        }
        Ok(&self.stdout)
    }
}

/// Run a program with a hard timeout.
///
/// Nothing the daemon spawns is allowed to hang it: a wedged `apt` or a `ffmpeg`
/// waiting on stdin would otherwise take the whole session with it.
pub fn run(program: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {}", program, e))?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(CmdOutput {
                        status: -1,
                        stdout: String::new(),
                        stderr: format!("{} exceeded {:?}", program, timeout),
                        timed_out: true,
                    });
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(format!("cannot wait on {}: {}", program, e)),
        }
    }
    let out = child.wait_with_output().map_err(|e| format!("cannot collect output: {}", e))?;
    Ok(CmdOutput {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        timed_out: false,
    })
}

pub fn have(program: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|d| std::path::Path::new(d).join(program).is_file())
}

pub fn systemctl(args: &[&str], _ctx: &ExecCtx) -> Result<String, String> {
    if !have("systemctl") {
        return Err("systemctl is not available on this machine".to_string());
    }
    let out = run("systemctl", args, Duration::from_secs(20))?;
    // `systemctl is-active` exits non-zero for inactive units, which is data
    // rather than failure, so status queries read stdout regardless.
    if args.first() == Some(&"is-active") || args.first() == Some(&"show") {
        return Ok(out.stdout);
    }
    out.require("systemctl").map(|s| s.to_string())
}

// ------------------------------------------------------------------ readouts

fn read_line_file(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default().trim().to_string()
}

pub fn sys_info() -> Json {
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let distro = os_release
        .lines()
        .find(|l| l.starts_with("PRETTY_NAME="))
        .and_then(|l| l.split_once('='))
        .map(|(_, v)| v.trim_matches('"').to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let uptime = read_line_file("/proc/uptime")
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    json_obj([
        ("name", nous_core::NOUS_NAME.into()),
        ("version", nous_core::NOUS_VERSION.into()),
        ("hostname", read_line_file("/proc/sys/kernel/hostname").into()),
        ("kernel", read_line_file("/proc/sys/kernel/osrelease").into()),
        ("distro", distro.into()),
        ("arch", std::env::consts::ARCH.into()),
        ("uptime_secs", (uptime as u64).into()),
        ("cpus", num_cpus().into()),
    ])
}

pub fn num_cpus() -> u64 {
    std::fs::read_to_string("/proc/cpuinfo")
        .map(|s| s.lines().filter(|l| l.starts_with("processor")).count() as u64)
        .unwrap_or(1)
        .max(1)
}

/// Parse `/proc/meminfo` into kilobyte values.
fn meminfo() -> std::collections::BTreeMap<String, u64> {
    let mut m = std::collections::BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once(':') {
                if let Some(kb) = v.trim().split_whitespace().next().and_then(|n| n.parse().ok()) {
                    m.insert(k.to_string(), kb);
                }
            }
        }
    }
    m
}

pub fn sys_metrics() -> Json {
    let load = read_line_file("/proc/loadavg");
    let mut parts = load.split_whitespace();
    let l1: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let l5: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let l15: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);

    let mi = meminfo();
    let total = mi.get("MemTotal").copied().unwrap_or(0);
    let avail = mi.get("MemAvailable").copied().unwrap_or(0);
    let used_pct = if total > 0 { ((total - avail) as f64 / total as f64) * 100.0 } else { 0.0 };

    let (disk_total, disk_free) = disk_usage("/");
    let disk_pct =
        if disk_total > 0 { ((disk_total - disk_free) as f64 / disk_total as f64) * 100.0 } else { 0.0 };

    json_obj([
        ("load1", l1.into()),
        ("load5", l5.into()),
        ("load15", l15.into()),
        ("cpus", num_cpus().into()),
        ("mem_total_kb", total.into()),
        ("mem_available_kb", avail.into()),
        ("mem_used_pct", (((used_pct * 10.0).round()) / 10.0).into()),
        ("disk_total_kb", disk_total.into()),
        ("disk_free_kb", disk_free.into()),
        ("disk_used_pct", (((disk_pct * 10.0).round()) / 10.0).into()),
        ("procs", count_procs().into()),
    ])
}

/// Total and free bytes for the filesystem containing `path`, via `statvfs`.
///
/// Declared here rather than pulled from a crate: it is one libc call and the
/// core has no dependencies.
pub fn disk_usage(path: &str) -> (u64, u64) {
    #[repr(C)]
    #[derive(Default)]
    struct StatVfs {
        f_bsize: u64,
        f_frsize: u64,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_favail: u64,
        f_fsid: u64,
        f_flag: u64,
        f_namemax: u64,
        f_spare: [u64; 6],
    }
    extern "C" {
        fn statvfs(path: *const u8, buf: *mut StatVfs) -> i32;
    }
    let mut c = Vec::with_capacity(path.len() + 1);
    c.extend_from_slice(path.as_bytes());
    c.push(0);
    let mut st = StatVfs::default();
    // SAFETY: `c` is NUL-terminated and `st` is a correctly sized, owned buffer.
    let rc = unsafe { statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return (0, 0);
    }
    let unit = if st.f_frsize > 0 { st.f_frsize } else { st.f_bsize };
    ((st.f_blocks * unit) / 1024, (st.f_bavail * unit) / 1024)
}

fn count_procs() -> u64 {
    std::fs::read_dir("/proc")
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()))
                .count() as u64
        })
        .unwrap_or(0)
}

fn proc_list(step: &Step) -> Result<Effect, String> {
    let limit = step.args.get("limit").and_then(|v| v.as_u64()).unwrap_or(40) as usize;
    let filter = step.args.str_or("filter", "").to_ascii_lowercase();
    let ticks_per_sec = 100.0; // USER_HZ; constant on every Linux target we support.

    let mut procs: Vec<(f64, Json)> = Vec::new();
    let entries = std::fs::read_dir("/proc").map_err(|e| format!("cannot read /proc: {}", e))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let pid: u64 = match name.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let stat = match std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
            Ok(s) => s,
            // The process exited between readdir and open. Normal; skip it.
            Err(_) => continue,
        };
        // The comm field is parenthesised and may itself contain spaces, so the
        // fields after it are located relative to the closing paren.
        let close = match stat.rfind(')') {
            Some(i) => i,
            None => continue,
        };
        let comm = stat[stat.find('(').map(|i| i + 1).unwrap_or(0)..close].to_string();
        let rest: Vec<&str> = stat[close + 2..].split_whitespace().collect();
        let utime: f64 = rest.get(11).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let stime: f64 = rest.get(12).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let rss_pages: f64 = rest.get(21).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let cpu_secs = (utime + stime) / ticks_per_sec;

        let cmdline = std::fs::read_to_string(format!("/proc/{}/cmdline", pid))
            .unwrap_or_default()
            .replace('\0', " ")
            .trim()
            .to_string();

        if !filter.is_empty()
            && !comm.to_ascii_lowercase().contains(&filter)
            && !cmdline.to_ascii_lowercase().contains(&filter)
        {
            continue;
        }

        procs.push((
            cpu_secs,
            json_obj([
                ("pid", pid.into()),
                ("name", comm.into()),
                ("cmdline", cmdline.into()),
                ("cpu_secs", ((cpu_secs * 100.0).round() / 100.0).into()),
                ("rss_kb", ((rss_pages * 4096.0) as u64 / 1024).into()),
                ("state", rest.first().copied().unwrap_or("?").into()),
            ]),
        ));
    }

    procs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let total = procs.len();
    let list: Vec<Json> = procs.into_iter().take(limit).map(|(_, j)| j).collect();
    Ok(Effect::read_only(
        json_obj([("processes", Json::Arr(list)), ("total", total.into())]),
        format!("listed {} of {} processes", limit.min(total), total),
    ))
}

fn proc_signal(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let pid = step.args.get("pid").and_then(|v| v.as_u64()).ok_or("step is missing 'pid'")?;
    let signal = step.args.str_or("signal", "TERM").to_string();
    if ctx.dry_run {
        return Ok(Effect::read_only(
            json_obj([("pid", pid.into()), ("signal", signal.clone().into())]),
            format!("would send SIG{} to pid {}", signal, pid),
        ));
    }
    let out = run("kill", &[&format!("-{}", signal), &pid.to_string()], Duration::from_secs(5))?;
    out.require("kill")?;
    Ok(Effect::with_undo(
        json_obj([("pid", pid.into()), ("signal", signal.clone().into())]),
        Undo::Manual { note: format!("pid {} was signalled; restart it if it was wanted", pid) },
        format!("sent SIG{} to pid {}", signal, pid),
    ))
}

fn svc_status(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let unit = step.args.str_or("unit", cap_scope(step)).to_string();
    if unit.is_empty() {
        return Err("step is missing 'unit'".to_string());
    }
    let active = systemctl(&["is-active", &unit], ctx).unwrap_or_default().trim().to_string();
    let enabled = systemctl(&["is-enabled", &unit], ctx).unwrap_or_default().trim().to_string();
    Ok(Effect::read_only(
        json_obj([
            ("unit", unit.clone().into()),
            ("active", active.clone().into()),
            ("enabled", enabled.into()),
            ("running", (active == "active").into()),
        ]),
        format!("{} is {}", unit, if active.is_empty() { "unknown" } else { &active }),
    ))
}

fn cap_scope(step: &Step) -> &str {
    step.capability.split_once(':').map(|(_, s)| s).unwrap_or("")
}

fn svc_change(cap: &Capability, step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let unit = step.args.str_or("unit", cap_scope(step)).to_string();
    if unit.is_empty() {
        return Err("step is missing 'unit'".to_string());
    }
    let was_active =
        systemctl(&["is-active", &unit], ctx).unwrap_or_default().trim() == "active";

    if ctx.dry_run {
        return Ok(Effect::read_only(
            json_obj([("unit", unit.clone().into()), ("action", cap.action.clone().into())]),
            format!("would {} {}", cap.action, unit),
        ));
    }
    systemctl(&[cap.action.as_str(), &unit], ctx)?;
    Ok(Effect::with_undo(
        json_obj([("unit", unit.clone().into()), ("action", cap.action.clone().into())]),
        Undo::ServiceState { unit: unit.clone(), was_active },
        format!("{}ed {}", cap.action.trim_end_matches('e'), unit),
    ))
}

/// Which package manager this machine actually has.
pub fn package_manager() -> Option<(&'static str, &'static str)> {
    for (bin, family) in
        [("apt-get", "debian"), ("dnf", "fedora"), ("pacman", "arch"), ("zypper", "suse")]
    {
        if have(bin) {
            return Some((bin, family));
        }
    }
    None
}

fn pkg_query(step: &Step, _ctx: &ExecCtx) -> Result<Effect, String> {
    let name = step.args.str_or("name", cap_scope(step)).to_string();
    if name.is_empty() {
        return Err("step is missing 'name'".to_string());
    }
    let (installed, version) = if have("dpkg-query") {
        let out = run("dpkg-query", &["-W", "-f=${Version}", &name], Duration::from_secs(15))?;
        (out.ok(), out.stdout.trim().to_string())
    } else if have("rpm") {
        let out = run("rpm", &["-q", "--qf", "%{VERSION}", &name], Duration::from_secs(15))?;
        (out.ok(), out.stdout.trim().to_string())
    } else {
        (false, String::new())
    };
    Ok(Effect::read_only(
        json_obj([
            ("name", name.clone().into()),
            ("installed", installed.into()),
            ("version", version.into()),
        ]),
        format!("{} is {}installed", name, if installed { "" } else { "not " }),
    ))
}

fn pkg_change(cap: &Capability, step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let name = step.args.str_or("name", cap_scope(step)).to_string();
    if name.is_empty() {
        return Err("step is missing 'name'".to_string());
    }
    let (bin, family) = package_manager().ok_or("no supported package manager on this machine")?;
    let installing = cap.action == "install";
    let args: Vec<String> = match (family, installing) {
        ("debian", true) => vec!["install".into(), "-y".into(), name.clone()],
        ("debian", false) => vec!["remove".into(), "-y".into(), name.clone()],
        ("fedora", true) => vec!["install".into(), "-y".into(), name.clone()],
        ("fedora", false) => vec!["remove".into(), "-y".into(), name.clone()],
        ("arch", true) => vec!["-S".into(), "--noconfirm".into(), name.clone()],
        ("arch", false) => vec!["-R".into(), "--noconfirm".into(), name.clone()],
        (_, true) => vec!["install".into(), "-y".into(), name.clone()],
        (_, false) => vec!["remove".into(), "-y".into(), name.clone()],
    };

    if ctx.dry_run {
        return Ok(Effect::read_only(
            json_obj([("name", name.clone().into()), ("manager", bin.into())]),
            format!("would run {} {}", bin, args.join(" ")),
        ));
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = run(bin, &refs, Duration::from_secs(600))?;
    out.require(bin)?;
    Ok(Effect::with_undo(
        json_obj([("name", name.clone().into()), ("manager", bin.into())]),
        // The inverse of an install is a remove, and vice versa.
        Undo::Manual {
            note: format!("run: {} {} {}", bin, if installing { "remove" } else { "install" }, name),
        },
        format!("{}ed {}", cap.action.trim_end_matches('e'), name),
    ))
}

fn net_status(_ctx: &ExecCtx) -> Json {
    let mut ifaces = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/sys/class/net") {
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let state = read_line_file(&format!("/sys/class/net/{}/operstate", name));
            let rx: u64 = read_line_file(&format!("/sys/class/net/{}/statistics/rx_bytes", name))
                .parse()
                .unwrap_or(0);
            let tx: u64 = read_line_file(&format!("/sys/class/net/{}/statistics/tx_bytes", name))
                .parse()
                .unwrap_or(0);
            ifaces.push(json_obj([
                ("name", name.clone().into()),
                ("state", state.clone().into()),
                ("up", (state == "up").into()),
                ("rx_bytes", rx.into()),
                ("tx_bytes", tx.into()),
                ("loopback", (name == "lo").into()),
            ]));
        }
    }
    let online = ifaces
        .iter()
        .any(|i| i.bool_or("up", false) && !i.bool_or("loopback", false));
    json_obj([("interfaces", Json::Arr(ifaces)), ("online", online.into())])
}

fn shell_exec(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let cmd = step.args.str_or("command", "").to_string();
    if cmd.trim().is_empty() {
        return Err("step is missing 'command'".to_string());
    }
    let timeout = Duration::from_secs(step.args.get("timeout_secs").and_then(|v| v.as_u64()).unwrap_or(60));

    if ctx.dry_run {
        return Ok(Effect::read_only(
            json_obj([("command", cmd.clone().into())]),
            format!("would run: {}", cmd),
        ));
    }
    let out = run("/bin/sh", &["-c", &cmd], timeout)?;
    Ok(Effect::with_undo(
        json_obj([
            ("command", cmd.clone().into()),
            ("status", (out.status as i64).into()),
            ("stdout", out.stdout.clone().into()),
            ("stderr", out.stderr.clone().into()),
            ("timed_out", out.timed_out.into()),
        ]),
        // The system cannot know how to reverse an arbitrary command, and must
        // not pretend otherwise.
        Undo::Manual { note: format!("`{}` ran; its effects are not tracked", cmd) },
        format!("ran `{}` (exit {})", cmd, out.status),
    ))
}

fn power(step: &Step, ctx: &ExecCtx) -> Result<Effect, String> {
    let mode = step.args.str_or("mode", "poweroff").to_string();
    let verb = match mode.as_str() {
        "reboot" | "restart" => "reboot",
        "suspend" => "suspend",
        "hibernate" => "hibernate",
        _ => "poweroff",
    };
    if ctx.dry_run {
        return Ok(Effect::read_only(
            json_obj([("mode", verb.into())]),
            format!("would {} the machine", verb),
        ));
    }
    systemctl(&[verb], ctx)?;
    Ok(Effect::with_undo(
        json_obj([("mode", verb.into())]),
        Undo::Manual { note: "power the machine back on".to_string() },
        format!("{} requested", verb),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::{Config, Journal};
    use std::path::PathBuf;

    fn fixture(tag: &str) -> (PathBuf, Config, Journal) {
        let dir = std::env::temp_dir().join(format!("nous-sys-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let j = Journal::open(&dir).unwrap();
        (dir, Config::with_defaults(), j)
    }

    #[test]
    fn describes_the_machine_it_is_running_on() {
        let info = sys_info();
        assert!(!info.str_or("kernel", "").is_empty(), "kernel version should be readable");
        assert!(info.get("cpus").unwrap().as_u64().unwrap() >= 1);
        assert_eq!(info.str_or("name", ""), "NOUS");
    }

    #[test]
    fn metrics_are_plausible() {
        let m = sys_metrics();
        let pct = m.f64_or("mem_used_pct", -1.0);
        assert!((0.0..=100.0).contains(&pct), "mem_used_pct out of range: {pct}");
        assert!(m.get("disk_total_kb").unwrap().as_u64().unwrap() > 0, "root fs should have a size");
        assert!(m.f64_or("load1", -1.0) >= 0.0);
    }

    #[test]
    fn lists_processes_including_this_one() {
        let (dir, cfg, j) = fixture("proc");
        let ctx = ExecCtx::rooted(&cfg, &j, false, dir.clone(), dir.clone());
        let step = Step::new("s", "proc.list", "sys", "", json_obj([("limit", 500u64.into())]));
        let e = execute(&Capability::parse("proc.list").unwrap(), &step, &ctx).unwrap();
        let procs = e.result.arr_or_empty("processes");
        assert!(!procs.is_empty());
        let me = std::process::id() as u64;
        assert!(
            procs.iter().any(|p| p.get("pid").and_then(|v| v.as_u64()) == Some(me)),
            "the test process should appear in its own process list"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn process_filter_narrows_the_list() {
        let (dir, cfg, j) = fixture("procfilter");
        let ctx = ExecCtx::rooted(&cfg, &j, false, dir.clone(), dir.clone());
        let step = Step::new(
            "s",
            "proc.list",
            "sys",
            "",
            json_obj([("filter", "definitely-not-a-real-process-xyz".into())]),
        );
        let e = execute(&Capability::parse("proc.list").unwrap(), &step, &ctx).unwrap();
        assert!(e.result.arr_or_empty("processes").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shell_exec_captures_output_and_status() {
        let (dir, cfg, j) = fixture("shell");
        let ctx = ExecCtx::rooted(&cfg, &j, false, dir.clone(), dir.clone());
        let step = Step::new(
            "s",
            "shell.exec",
            "sys",
            "",
            json_obj([("command", "echo hello; exit 3".into())]),
        );
        let e = execute(&Capability::parse("shell.exec").unwrap(), &step, &ctx).unwrap();
        assert_eq!(e.result.str_or("stdout", "").trim(), "hello");
        assert_eq!(e.result.get("status").unwrap().as_i64(), Some(3));
        // An arbitrary command cannot be auto-reversed, and says so.
        assert!(matches!(e.undo, Undo::Manual { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hung_command_is_killed_rather_than_hanging_the_daemon() {
        let out = run("/bin/sh", &["-c", "sleep 30"], Duration::from_millis(300)).unwrap();
        assert!(out.timed_out, "the watchdog should have fired");
        assert!(out.require("sleep").is_err());
    }

    #[test]
    fn dry_run_never_touches_the_machine() {
        let (dir, cfg, j) = fixture("shelldry");
        let ctx = ExecCtx::rooted(&cfg, &j, true, dir.clone(), dir.clone());
        let marker = dir.join("should-not-exist");
        let step = Step::new(
            "s",
            "shell.exec",
            "sys",
            "",
            json_obj([("command", format!("touch {}", marker.display()).into())]),
        );
        let e = execute(&Capability::parse("shell.exec").unwrap(), &step, &ctx).unwrap();
        assert!(!marker.exists(), "dry run must not run the command");
        assert!(e.detail.starts_with("would run"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn network_status_sees_at_least_loopback() {
        let (dir, cfg, j) = fixture("net");
        let ctx = ExecCtx::rooted(&cfg, &j, false, dir.clone(), dir.clone());
        let n = net_status(&ctx);
        let ifaces = n.arr_or_empty("interfaces");
        assert!(ifaces.iter().any(|i| i.str_or("name", "") == "lo"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_usage_reads_the_root_filesystem() {
        let (total, free) = disk_usage("/");
        assert!(total > 0);
        assert!(free <= total);
        assert_eq!(disk_usage("/definitely/not/a/path"), (0, 0));
    }
}
