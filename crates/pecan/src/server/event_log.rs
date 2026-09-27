//! Sequenced server event bus with a bounded replay window.
//!
//! Every published event gets a monotonically increasing sequence number and
//! is kept in a ring buffer. A reconnecting client passes the last sequence it
//! saw and receives exactly the events it missed, or a `reset` when the gap is
//! older than the window and it must refetch.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use super::snapshot::ServerEvent;

/// Events kept for replay. Streaming emits one event per text delta, so this
/// covers a few minutes of a busy session while staying well under a few MB.
const REPLAY_CAPACITY: usize = 4096;
/// Live fan-out buffer per subscriber before it is considered lagged.
const LIVE_CAPACITY: usize = 1024;

/// One published event and its sequence number.
#[derive(Debug, Clone)]
pub(crate) struct Sequenced {
    /// Monotonic sequence, starting at 1.
    pub(crate) seq: u64,
    /// The event payload.
    pub(crate) event: Arc<ServerEvent>,
}

#[derive(Debug)]
struct Ring {
    next_seq: u64,
    events: VecDeque<Sequenced>,
}

/// Shared publisher/subscriber handle.
#[derive(Debug, Clone)]
pub(crate) struct EventLog {
    ring: Arc<Mutex<Ring>>,
    live: broadcast::Sender<Sequenced>,
    capacity: usize,
}

/// A subscription: what was missed plus the live tail after it.
#[derive(Debug)]
pub(crate) struct Subscription {
    /// Events after the requested cursor, oldest first.
    pub(crate) replay: Vec<Sequenced>,
    /// True when the cursor fell outside the replay window (client must
    /// refetch its state instead of trusting the replay).
    pub(crate) reset: bool,
    /// Sequence of the newest published event at subscribe time.
    pub(crate) head: u64,
    /// Live events published after `head`.
    pub(crate) live: broadcast::Receiver<Sequenced>,
}

impl EventLog {
    /// Creates an empty log with the default replay window.
    pub(crate) fn new() -> Self {
        Self::with_capacity(REPLAY_CAPACITY)
    }

    fn with_capacity(capacity: usize) -> Self {
        let (live, _) = broadcast::channel(LIVE_CAPACITY);
        Self {
            ring: Arc::new(Mutex::new(Ring {
                next_seq: 1,
                events: VecDeque::with_capacity(capacity.min(256)),
            })),
            live,
            capacity,
        }
    }

    /// Publishes one event to the replay window and live subscribers.
    pub(crate) fn send(&self, event: ServerEvent) {
        let Ok(mut ring) = self.ring.lock() else {
            tracing::error!("event log mutex poisoned; dropping event");
            return;
        };
        let item = Sequenced { seq: ring.next_seq, event: Arc::new(event) };
        ring.next_seq = ring.next_seq.saturating_add(1);
        if ring.events.len() >= self.capacity {
            ring.events.pop_front();
        }
        ring.events.push_back(item.clone());
        // Sending under the lock keeps live order identical to seq order and
        // makes subscribe's snapshot+receiver pair gap-free.
        self.live.send(item).unwrap_or_default();
    }

    /// Subscribes after `after` (the last sequence the client saw).
    ///
    /// `None` means a fresh client: no replay, no reset.
    pub(crate) fn subscribe(&self, after: Option<u64>) -> Subscription {
        let Ok(ring) = self.ring.lock() else {
            tracing::error!("event log mutex poisoned; forcing client reset");
            return Subscription {
                replay: Vec::new(),
                reset: true,
                head: 0,
                live: self.live.subscribe(),
            };
        };
        let live = self.live.subscribe();
        let head = ring.next_seq.saturating_sub(1);
        let oldest = ring.events.front().map_or(ring.next_seq, |item| item.seq);
        let (replay, reset) = match after {
            None => (Vec::new(), false),
            // A cursor from the future means the server restarted.
            Some(cursor) if cursor > head => (Vec::new(), true),
            Some(cursor) if cursor.saturating_add(1) < oldest => (Vec::new(), true),
            Some(cursor) => {
                (ring.events.iter().filter(|item| item.seq > cursor).cloned().collect(), false)
            }
        };
        Subscription { replay, reset, head, live }
    }
}

#[cfg(test)]
mod tests {
    use super::EventLog;
    use crate::server::snapshot::ServerEvent;

    fn changed(id: &str) -> ServerEvent {
        ServerEvent::ThreadChanged { id: id.to_owned() }
    }

    #[test]
    fn fresh_subscriber_gets_no_replay_and_live_tail() {
        let log = EventLog::with_capacity(8);
        log.send(changed("a"));
        let mut sub = log.subscribe(None);
        assert!(sub.replay.is_empty(), "fresh client needs no replay");
        assert!(!sub.reset, "fresh client is not a reset");
        assert_eq!(sub.head, 1, "head is the newest seq");
        log.send(changed("b"));
        let live = sub.live.try_recv();
        assert!(matches!(live, Ok(ref item) if item.seq == 2), "live tail continues at 2");
    }

    #[test]
    fn replays_exactly_the_missed_events() {
        let log = EventLog::with_capacity(8);
        for id in ["a", "b", "c", "d"] {
            log.send(changed(id));
        }
        let sub = log.subscribe(Some(2));
        let seqs: Vec<u64> = sub.replay.iter().map(|item| item.seq).collect();
        assert_eq!(seqs, vec![3, 4], "replay starts after the cursor");
        assert!(!sub.reset, "cursor inside window is not a reset");
    }

    #[test]
    fn caught_up_cursor_replays_nothing() {
        let log = EventLog::with_capacity(8);
        log.send(changed("a"));
        let sub = log.subscribe(Some(1));
        assert!(sub.replay.is_empty() && !sub.reset, "caught-up client resumes cleanly");
    }

    #[test]
    fn cursor_older_than_window_forces_reset() {
        let log = EventLog::with_capacity(2);
        for id in ["a", "b", "c", "d"] {
            log.send(changed(id));
        }
        // Window holds 3 and 4; a client at 1 missed 2, which is gone.
        let sub = log.subscribe(Some(1));
        assert!(sub.reset, "evicted gap must reset");
        assert!(sub.replay.is_empty(), "no partial replay on reset");
        // A client at 2 missed only 3 and 4, both still held.
        let sub = log.subscribe(Some(2));
        assert!(!sub.reset, "gap fully inside window replays");
        assert_eq!(sub.replay.len(), 2, "both retained events replay");
    }

    #[test]
    fn cursor_from_a_previous_server_forces_reset() {
        let log = EventLog::with_capacity(8);
        log.send(changed("a"));
        let sub = log.subscribe(Some(500));
        assert!(sub.reset, "future cursor means the server restarted");
    }

    #[test]
    fn empty_log_accepts_zero_cursor() {
        let log = EventLog::with_capacity(8);
        let sub = log.subscribe(Some(0));
        assert!(!sub.reset && sub.replay.is_empty(), "nothing missed on an empty log");
    }
}
