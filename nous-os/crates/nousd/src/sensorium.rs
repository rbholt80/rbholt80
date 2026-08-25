//! The sensorium: what the system notices about itself.
//!
//! A background thread samples the machine and publishes what it finds. This is
//! the difference between an assistant you have to ask and an OS that already
//! knows — the curator's proposals and the shell's "your disk is nearly full"
//! both come from here, and neither costs an inference.

use crate::bus::Bus;
use crate::exec::sysops;
use nous_core::journal::now_secs;
use nous_core::json::{json_obj, Json};
use nous_core::proto::topic;
use nous_core::{Config, Event};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A condition worth surfacing to the user.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub kind: &'static str,
    pub severity: u8,
    pub message: String,
}

impl Alert {
    fn to_json(&self) -> Json {
        json_obj([
            ("kind", self.kind.into()),
            ("severity", (self.severity as u64).into()),
            ("message", self.message.clone().into()),
        ])
    }
}

/// Decide what, if anything, is worth saying about a metrics sample.
///
/// Split out from the sampling loop so the thresholds are testable without
/// waiting for a real machine to fill its disk.
pub fn evaluate(metrics: &Json, cfg: &Config) -> Vec<Alert> {
    let mut out = Vec::new();

    let disk_pct = metrics.f64_or("disk_used_pct", 0.0);
    let disk_threshold = cfg.f64_or("sensor.disk_alert_pct", 92.0);
    if disk_pct >= disk_threshold {
        let free_gb = metrics
            .get("disk_free_kb")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            / 1024
            / 1024;
        out.push(Alert {
            kind: "disk",
            severity: if disk_pct >= 97.0 { 4 } else { 3 },
            message: format!("Disk is {:.0}% full — {} GB left.", disk_pct, free_gb),
        });
    }

    let mem_pct = metrics.f64_or("mem_used_pct", 0.0);
    if mem_pct >= cfg.f64_or("sensor.mem_alert_pct", 90.0) {
        out.push(Alert {
            kind: "memory",
            severity: 3,
            message: format!("Memory is {:.0}% used.", mem_pct),
        });
    }

    // Load is only meaningful relative to core count, so normalise it.
    let cpus = metrics
        .get("cpus")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1) as f64;
    let load = metrics.f64_or("load1", 0.0);
    if load / cpus >= cfg.f64_or("sensor.load_alert", 4.0) {
        out.push(Alert {
            kind: "load",
            severity: 2,
            message: format!("Load is {:.1} across {} cores.", load, cpus as u64),
        });
    }

    out
}

pub struct Sensorium {
    bus: Arc<Bus>,
    cfg: Config,
    running: Arc<AtomicBool>,
}

impl Sensorium {
    pub fn new(bus: Arc<Bus>, cfg: Config) -> Sensorium {
        Sensorium {
            bus,
            cfg,
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Sample once and publish. Returns the alerts raised.
    pub fn tick(&self, last: &mut Vec<Alert>) -> Vec<Alert> {
        let metrics = sysops::sys_metrics();
        self.bus.publish(Event::new(
            topic::SENSOR,
            json_obj([
                ("kind", "metrics".into()),
                ("at", now_secs().into()),
                ("metrics", metrics.clone()),
            ]),
        ));

        let alerts = evaluate(&metrics, &self.cfg);
        for a in &alerts {
            // Only announce a condition when it first appears. Repeating "your
            // disk is full" every twenty seconds is how notifications get muted.
            if !last.iter().any(|p| p.kind == a.kind) {
                self.bus.publish(Event::new(
                    topic::NOTIFY,
                    json_obj([("source", "sensorium".into()), ("alert", a.to_json())]),
                ));
            }
        }
        *last = alerts.clone();
        alerts
    }

    /// Run the sampling loop until stopped.
    pub fn run(self) {
        let interval = Duration::from_secs(self.cfg.u64_or("sensor.interval_secs", 20).max(2));
        let mut last: Vec<Alert> = Vec::new();
        while self.running.load(Ordering::Relaxed) {
            self.tick(&mut last);
            // Wake often enough to shut down promptly without polling hard.
            let mut slept = Duration::ZERO;
            while slept < interval && self.running.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
                slept += Duration::from_millis(200);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(disk: f64, mem: f64, load: f64, cpus: u64) -> Json {
        json_obj([
            ("disk_used_pct", disk.into()),
            ("disk_free_kb", 1_048_576u64.into()),
            ("mem_used_pct", mem.into()),
            ("load1", load.into()),
            ("cpus", cpus.into()),
        ])
    }

    #[test]
    fn a_healthy_machine_raises_nothing() {
        let alerts = evaluate(&metrics(40.0, 50.0, 0.5, 8), &Config::with_defaults());
        assert!(alerts.is_empty(), "{alerts:?}");
    }

    #[test]
    fn a_full_disk_is_reported() {
        let alerts = evaluate(&metrics(95.0, 30.0, 0.2, 8), &Config::with_defaults());
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, "disk");
        assert_eq!(alerts[0].severity, 3);
    }

    #[test]
    fn a_nearly_dead_disk_escalates() {
        let alerts = evaluate(&metrics(98.0, 30.0, 0.2, 8), &Config::with_defaults());
        assert_eq!(alerts[0].severity, 4);
    }

    #[test]
    fn load_is_judged_against_core_count() {
        let cfg = Config::with_defaults();
        // 8.0 on 8 cores is a busy machine, not a broken one.
        assert!(evaluate(&metrics(10.0, 10.0, 8.0, 8), &cfg).is_empty());
        // The same load on one core is worth mentioning.
        let alerts = evaluate(&metrics(10.0, 10.0, 8.0, 1), &cfg);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, "load");
    }

    #[test]
    fn thresholds_are_configurable() {
        let mut cfg = Config::with_defaults();
        cfg.set("sensor.disk_alert_pct", "50");
        let alerts = evaluate(&metrics(60.0, 10.0, 0.1, 8), &cfg);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, "disk");
    }

    #[test]
    fn several_problems_are_all_reported() {
        let alerts = evaluate(&metrics(99.0, 95.0, 40.0, 4), &Config::with_defaults());
        let kinds: Vec<&str> = alerts.iter().map(|a| a.kind).collect();
        assert_eq!(kinds, ["disk", "memory", "load"]);
    }

    #[test]
    fn a_condition_is_announced_once_not_every_tick() {
        let bus = Arc::new(Bus::new());
        let (_id, rx) = bus.subscribe(vec![topic::NOTIFY.to_string()]);
        let mut cfg = Config::with_defaults();
        // Guarantee an alert on this machine whatever its real disk usage.
        cfg.set("sensor.disk_alert_pct", "0");
        let s = Sensorium::new(bus, cfg);

        let mut last = Vec::new();
        s.tick(&mut last);
        assert!(rx.try_recv().is_ok(), "the first occurrence should notify");
        s.tick(&mut last);
        assert!(
            rx.try_recv().is_err(),
            "a standing condition must not notify again"
        );
    }

    #[test]
    fn metrics_are_published_every_tick_regardless() {
        let bus = Arc::new(Bus::new());
        let (_id, rx) = bus.subscribe(vec![topic::SENSOR.to_string()]);
        let s = Sensorium::new(bus, Config::with_defaults());
        let mut last = Vec::new();
        s.tick(&mut last);
        s.tick(&mut last);
        assert_eq!(rx.try_iter().count(), 2);
    }
}
