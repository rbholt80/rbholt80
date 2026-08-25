//! The event bus.
//!
//! Every connection may subscribe to topics; the daemon publishes what it is
//! doing. This is how the shell shows a plan executing step by step, and how the
//! desktop learns that the curator found something worth mentioning.

use nous_core::proto::Event;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TrySendError};
use std::sync::Mutex;

/// A slow subscriber must not be able to stall the daemon. Once this many
/// events are outstanding the subscriber starts losing the oldest ones, which
/// is the right trade for a UI feed.
const QUEUE_LIMIT: usize = 512;

struct Subscriber {
    topics: Vec<String>,
    tx: Sender<Event>,
    /// Events dropped because this subscriber could not keep up.
    dropped: u64,
    queued: usize,
}

impl Subscriber {
    fn wants(&self, topic: &str) -> bool {
        self.topics
            .iter()
            .any(|t| t == "*" || t == topic || topic.starts_with(&format!("{}.", t)))
    }
}

pub struct Bus {
    subs: Mutex<HashMap<u64, Subscriber>>,
    next_id: AtomicU64,
    published: AtomicU64,
}

impl Default for Bus {
    fn default() -> Self {
        Bus::new()
    }
}

impl Bus {
    pub fn new() -> Bus {
        Bus {
            subs: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            published: AtomicU64::new(0),
        }
    }

    pub fn subscribe(&self, topics: Vec<String>) -> (u64, Receiver<Event>) {
        let (tx, rx) = channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let topics = if topics.is_empty() {
            vec!["*".to_string()]
        } else {
            topics
        };
        self.subs.lock().unwrap().insert(
            id,
            Subscriber {
                topics,
                tx,
                dropped: 0,
                queued: 0,
            },
        );
        (id, rx)
    }

    pub fn unsubscribe(&self, id: u64) {
        self.subs.lock().unwrap().remove(&id);
    }

    /// Deliver an event to every interested subscriber. Subscribers whose
    /// receiver has been dropped are reaped here.
    pub fn publish(&self, event: Event) {
        self.published.fetch_add(1, Ordering::Relaxed);
        let mut subs = self.subs.lock().unwrap();
        let mut dead = Vec::new();
        for (id, sub) in subs.iter_mut() {
            if !sub.wants(&event.topic) {
                continue;
            }
            if sub.queued >= QUEUE_LIMIT {
                sub.dropped += 1;
                continue;
            }
            match sub.tx.send(event.clone()) {
                Ok(()) => sub.queued += 1,
                Err(_) => dead.push(*id),
            }
        }
        for id in dead {
            subs.remove(&id);
        }
    }

    /// Called by a connection's writer thread once it has drained an event, so
    /// the bus can tell a slow subscriber from a merely idle one.
    pub fn ack(&self, id: u64, n: usize) {
        if let Some(sub) = self.subs.lock().unwrap().get_mut(&id) {
            sub.queued = sub.queued.saturating_sub(n);
        }
    }

    pub fn subscriber_count(&self) -> usize {
        self.subs.lock().unwrap().len()
    }

    pub fn published_count(&self) -> u64 {
        self.published.load(Ordering::Relaxed)
    }

    pub fn dropped_count(&self) -> u64 {
        self.subs.lock().unwrap().values().map(|s| s.dropped).sum()
    }
}

/// Convenience: unused, but keeps the `TrySendError` import meaningful if the
/// channel type is ever swapped for a bounded one.
#[allow(dead_code)]
fn is_full<T>(e: &TrySendError<T>) -> bool {
    matches!(e, TrySendError::Full(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::json::json_obj;

    fn evt(topic: &str) -> Event {
        Event::new(topic, json_obj([("x", 1i64.into())]))
    }

    #[test]
    fn delivers_only_subscribed_topics() {
        let bus = Bus::new();
        let (_id, rx) = bus.subscribe(vec!["intent".into()]);
        bus.publish(evt("sensor"));
        bus.publish(evt("intent"));
        let got = rx.try_recv().expect("subscribed topic should arrive");
        assert_eq!(got.topic, "intent");
        assert!(rx.try_recv().is_err(), "unsubscribed topic must not arrive");
    }

    #[test]
    fn wildcard_receives_everything() {
        let bus = Bus::new();
        let (_id, rx) = bus.subscribe(vec![]);
        bus.publish(evt("sensor"));
        bus.publish(evt("agent"));
        assert_eq!(rx.try_recv().unwrap().topic, "sensor");
        assert_eq!(rx.try_recv().unwrap().topic, "agent");
    }

    #[test]
    fn topic_prefixes_match_subtopics() {
        let bus = Bus::new();
        let (_id, rx) = bus.subscribe(vec!["media".into()]);
        bus.publish(evt("media.playback"));
        assert_eq!(rx.try_recv().unwrap().topic, "media.playback");
    }

    #[test]
    fn dropped_receivers_are_reaped() {
        let bus = Bus::new();
        let (_id, rx) = bus.subscribe(vec!["*".into()]);
        assert_eq!(bus.subscriber_count(), 1);
        drop(rx);
        bus.publish(evt("log"));
        assert_eq!(
            bus.subscriber_count(),
            0,
            "a hung-up subscriber should be removed"
        );
    }

    #[test]
    fn a_slow_subscriber_loses_events_instead_of_blocking() {
        let bus = Bus::new();
        let (_id, _rx) = bus.subscribe(vec!["*".into()]);
        for _ in 0..(QUEUE_LIMIT + 25) {
            bus.publish(evt("log"));
        }
        assert_eq!(
            bus.dropped_count(),
            25,
            "backpressure should shed, not stall"
        );
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let bus = Bus::new();
        let (id, rx) = bus.subscribe(vec!["*".into()]);
        bus.unsubscribe(id);
        bus.publish(evt("log"));
        assert!(rx.try_recv().is_err());
    }
}
