//! Hardware profiling.
//!
//! The reason this exists: "can my computer run this?" should not be a question
//! the user has to research. NOUS probes the machine at install time and again
//! at boot, picks a model routing profile that actually fits, and says plainly
//! what it chose and why.
//!
//! The OS layer itself is small — a static daemon and a compositor. What varies
//! is how much of the intelligence runs locally, and that is a spectrum, not a
//! requirement.

use crate::exec::sysops::{disk_usage, have, num_cpus, run};
use nous_core::json::{json_obj, Json};
use std::time::Duration;

/// How the machine should be configured to run its models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Below the floor for the graphical shell.
    Unsupported,
    /// Runs the OS well; intelligence comes from an API key. No local model.
    Hosted,
    /// Small local model for routine work, hosted model for the hard cases.
    /// This is the profile most laptops land in, and the one the system is
    /// designed around.
    Hybrid,
    /// A 7–8B local model handles everything. Nothing needs to leave the machine.
    Local,
    /// Enough memory or VRAM for a large local model.
    Workstation,
}

impl Profile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Profile::Unsupported => "unsupported",
            Profile::Hosted => "hosted",
            Profile::Hybrid => "hybrid",
            Profile::Local => "local",
            Profile::Workstation => "workstation",
        }
    }

    /// The `model.route` this profile implies.
    pub fn route(&self) -> &'static str {
        match self {
            // No local model to fall back to, so the API key is the only path.
            Profile::Unsupported | Profile::Hosted => "anthropic,openai,offline",
            Profile::Hybrid => "ollama,anthropic,openai,offline",
            Profile::Local | Profile::Workstation => "ollama,anthropic,openai,offline",
        }
    }

    /// The `model.route.small` this profile implies.
    pub fn small_route(&self) -> &'static str {
        match self {
            // Without a local model, routine work has nowhere private to go, so
            // it is simply not done rather than silently sent to an API.
            Profile::Unsupported | Profile::Hosted => "offline",
            _ => "ollama,offline",
        }
    }

    /// Which local model to pull, if any.
    pub fn local_model(&self) -> Option<&'static str> {
        match self {
            Profile::Unsupported | Profile::Hosted => None,
            Profile::Hybrid => Some("qwen2.5:1.5b-instruct"),
            Profile::Local => Some("qwen2.5:7b-instruct"),
            Profile::Workstation => Some("qwen2.5:14b-instruct"),
        }
    }

    pub fn explain(&self) -> &'static str {
        match self {
            Profile::Unsupported => {
                "Below the floor for the graphical shell. The daemon and the \
                 command shell still work; the desktop will not."
            }
            Profile::Hosted => {
                "Plenty for the desktop, files and media. Intelligence comes \
                 from your API key rather than from this machine."
            }
            Profile::Hybrid => {
                "A small local model handles routine work — naming, sorting, \
                 classifying — and never leaves the machine. Harder requests go \
                 to your API key."
            }
            Profile::Local => {
                "A 7B local model can handle everything, including intent \
                 resolution. An API key stays useful for the hardest requests \
                 but is no longer required."
            }
            Profile::Workstation => {
                "Enough headroom for a large local model. This machine can run \
                 the whole system with no network at all."
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Gpu {
    pub vendor: String,
    pub name: String,
    /// Video memory in MB. Zero when it could not be determined.
    pub vram_mb: u64,
}

#[derive(Debug, Clone)]
pub struct HardwareProfile {
    pub cpus: u64,
    pub cpu_model: String,
    pub ram_mb: u64,
    pub disk_free_mb: u64,
    pub gpus: Vec<Gpu>,
    pub arch: String,
    pub profile: Profile,
    /// Things worth telling the user before they buy anything.
    pub notes: Vec<String>,
}

/// Detect what this machine is.
pub fn detect() -> HardwareProfile {
    let ram_mb = total_ram_kb() / 1024;
    let (_, disk_free_kb) = disk_usage("/");
    let gpus = detect_gpus();
    let cpus = num_cpus();
    let best_vram = gpus.iter().map(|g| g.vram_mb).max().unwrap_or(0);

    let mut notes = Vec::new();
    let profile = classify(ram_mb, cpus, best_vram, &mut notes);

    if disk_free_kb / 1024 < 12_000 {
        notes.push(format!(
            "Only {} GB free on /. The system needs about 8 GB, plus 1–9 GB per local model.",
            disk_free_kb / 1024 / 1024
        ));
    }

    HardwareProfile {
        cpus,
        cpu_model: cpu_model(),
        ram_mb,
        disk_free_mb: disk_free_kb / 1024,
        gpus,
        arch: std::env::consts::ARCH.to_string(),
        profile,
        notes,
    }
}

/// Choose a profile from the numbers.
///
/// Separated from `detect` so the thresholds are testable without owning five
/// different computers.
pub fn classify(ram_mb: u64, cpus: u64, vram_mb: u64, notes: &mut Vec<String>) -> Profile {
    if ram_mb < 3_500 {
        notes.push(
            "Under 4 GB of RAM. The daemon and command shell run here, but the \
             graphical shell needs more."
                .to_string(),
        );
        return Profile::Unsupported;
    }
    if cpus < 2 {
        notes.push("A single core will feel slow, though it will work.".to_string());
    }

    // A discrete GPU changes the picture more than anything else.
    if vram_mb >= 20_000 || ram_mb >= 60_000 {
        return Profile::Workstation;
    }
    if vram_mb >= 7_000 || ram_mb >= 30_000 {
        return Profile::Local;
    }
    if ram_mb >= 15_000 {
        notes.push(
            "16 GB is enough for a 7B local model if you want one — set the \
             profile to `local` after installing."
                .to_string(),
        );
        return Profile::Hybrid;
    }
    if ram_mb >= 7_000 {
        return Profile::Hybrid;
    }
    notes.push(
        "Between 4 and 8 GB: the desktop is comfortable, but a local model \
         would crowd it. Use an API key instead."
            .to_string(),
    );
    Profile::Hosted
}

fn total_ram_kb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse().ok())
        })
        .unwrap_or(0)
}

fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("model name") || l.starts_with("Model"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Enumerate GPUs from sysfs, then ask the vendor tool for VRAM where possible.
fn detect_gpus() -> Vec<Gpu> {
    let mut out = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/sys/class/drm") {
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            // `card0`, not `card0-HDMI-A-1`.
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let vendor_id = std::fs::read_to_string(e.path().join("device/vendor"))
                .unwrap_or_default()
                .trim()
                .to_string();
            let vendor = match vendor_id.as_str() {
                "0x10de" => "nvidia",
                "0x1002" => "amd",
                "0x8086" => "intel",
                _ => "unknown",
            };
            let vram_mb = std::fs::read_to_string(e.path().join("device/mem_info_vram_total"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(|b| b / 1024 / 1024)
                .unwrap_or(0);
            out.push(Gpu { vendor: vendor.to_string(), name: name.clone(), vram_mb });
        }
    }

    // NVIDIA does not publish VRAM through sysfs, so ask its tool if present.
    if have("nvidia-smi") {
        if let Ok(r) = run(
            "nvidia-smi",
            &["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"],
            Duration::from_secs(5),
        ) {
            if r.ok() {
                for (i, line) in r.stdout.lines().enumerate() {
                    let mut parts = line.split(',');
                    let name = parts.next().unwrap_or("nvidia").trim().to_string();
                    let mb: u64 = parts.next().and_then(|m| m.trim().parse().ok()).unwrap_or(0);
                    match out.iter_mut().filter(|g| g.vendor == "nvidia").nth(i) {
                        Some(g) => {
                            g.name = name;
                            g.vram_mb = mb;
                        }
                        None => out.push(Gpu { vendor: "nvidia".into(), name, vram_mb: mb }),
                    }
                }
            }
        }
    }
    out
}

impl HardwareProfile {
    pub fn to_json(&self) -> Json {
        json_obj([
            ("profile", self.profile.as_str().into()),
            ("explain", self.profile.explain().into()),
            ("route", self.profile.route().into()),
            ("route_small", self.profile.small_route().into()),
            ("local_model", self.profile.local_model().map(Json::from).unwrap_or(Json::Null)),
            ("cpus", self.cpus.into()),
            ("cpu_model", self.cpu_model.clone().into()),
            ("ram_mb", self.ram_mb.into()),
            ("disk_free_mb", self.disk_free_mb.into()),
            ("arch", self.arch.clone().into()),
            (
                "gpus",
                Json::Arr(
                    self.gpus
                        .iter()
                        .map(|g| {
                            json_obj([
                                ("vendor", g.vendor.clone().into()),
                                ("name", g.name.clone().into()),
                                ("vram_mb", g.vram_mb.into()),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("notes", Json::Arr(self.notes.iter().map(|n| Json::Str(n.clone())).collect())),
        ])
    }

    /// A short report for `nousctl doctor`.
    pub fn report(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("  CPU     {} ({} cores, {})\n", self.cpu_model, self.cpus, self.arch));
        s.push_str(&format!("  Memory  {} MB\n", self.ram_mb));
        s.push_str(&format!("  Disk    {} MB free on /\n", self.disk_free_mb));
        if self.gpus.is_empty() {
            s.push_str("  GPU     none detected\n");
        } else {
            for g in &self.gpus {
                let vram = if g.vram_mb > 0 {
                    format!("{} MB VRAM", g.vram_mb)
                } else {
                    "VRAM unknown".to_string()
                };
                s.push_str(&format!("  GPU     {} ({}, {})\n", g.name, g.vendor, vram));
            }
        }
        s.push_str(&format!("\n  Profile {}\n", self.profile.as_str()));
        s.push_str(&format!("          {}\n", self.profile.explain()));
        if let Some(m) = self.profile.local_model() {
            s.push_str(&format!("          Local model: {}\n", m));
        }
        for n in &self.notes {
            s.push_str(&format!("\n  Note    {}\n", n));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify_only(ram_mb: u64, cpus: u64, vram_mb: u64) -> Profile {
        classify(ram_mb, cpus, vram_mb, &mut Vec::new())
    }

    #[test]
    fn a_netbook_is_below_the_graphical_floor() {
        assert_eq!(classify_only(2_048, 2, 0), Profile::Unsupported);
    }

    #[test]
    fn a_modest_laptop_uses_a_hosted_model() {
        // 4 GB: the desktop is fine, a local model is not.
        assert_eq!(classify_only(4_096, 2, 0), Profile::Hosted);
        assert_eq!(Profile::Hosted.local_model(), None);
    }

    #[test]
    fn eight_gigabytes_reaches_the_hybrid_profile() {
        let p = classify_only(8_192, 4, 0);
        assert_eq!(p, Profile::Hybrid);
        assert_eq!(p.local_model(), Some("qwen2.5:1.5b-instruct"));
    }

    #[test]
    fn a_discrete_gpu_unlocks_a_full_local_model() {
        // 16 GB of RAM alone is hybrid...
        assert_eq!(classify_only(16_384, 8, 0), Profile::Hybrid);
        // ...but an 8 GB card changes the answer.
        assert_eq!(classify_only(16_384, 8, 8_192), Profile::Local);
    }

    #[test]
    fn plenty_of_ram_also_reaches_local_without_a_gpu() {
        assert_eq!(classify_only(32_768, 8, 0), Profile::Local);
        assert_eq!(classify_only(64_512, 16, 0), Profile::Workstation);
        assert_eq!(classify_only(32_768, 16, 24_576), Profile::Workstation);
    }

    #[test]
    fn a_hosted_profile_never_sends_routine_work_to_an_api() {
        // Without a local model there is nowhere private to do small work, so
        // the small route stops rather than quietly billing the user.
        assert_eq!(Profile::Hosted.small_route(), "offline");
        assert_eq!(Profile::Hybrid.small_route(), "ollama,offline");
    }

    #[test]
    fn low_memory_and_low_disk_produce_advice() {
        let mut notes = Vec::new();
        classify(4_096, 2, 0, &mut notes);
        assert!(notes.iter().any(|n| n.contains("API key")), "{notes:?}");

        let mut single = Vec::new();
        classify(8_192, 1, 0, &mut single);
        assert!(single.iter().any(|n| n.contains("single core")), "{single:?}");
    }

    #[test]
    fn detection_describes_the_machine_it_runs_on() {
        let hw = detect();
        assert!(hw.ram_mb > 0, "should read total memory");
        assert!(hw.cpus >= 1);
        assert_ne!(hw.profile, Profile::Unsupported, "the test machine should clear the floor");
        assert!(hw.report().contains("Profile"));
        assert!(!hw.to_json().str_or("route", "").is_empty());
    }
}
