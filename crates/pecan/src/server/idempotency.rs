//! Replay-safe mutations for flaky mobile links.
//!
//! A client that lost the response to a prompt or dialog answer retries with
//! the same `Idempotency-Key`. The first successful result is remembered for a
//! while and returned again instead of re-running the side effect. Failures
//! are forgotten so a retry can succeed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a completed result stays replayable.
const RESULT_TTL: Duration = Duration::from_secs(10 * 60);
/// Upper bound on remembered keys.
const MAX_ENTRIES: usize = 1024;
/// Accepted key length range.
const KEY_LEN: std::ops::RangeInclusive<usize> = 8..=128;

#[derive(Debug, Clone)]
enum Entry {
    InFlight { started: Instant },
    Done { finished: Instant, body: serde_json::Value },
}

impl Entry {
    fn age_anchor(&self) -> Instant {
        match self {
            Self::InFlight { started } => *started,
            Self::Done { finished, .. } => *finished,
        }
    }
}

/// Shared idempotency registry.
#[derive(Debug, Clone, Default)]
pub(crate) struct Idempotency {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}

/// Why a key could not be claimed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum ClaimError {
    /// The key is malformed.
    #[error("Idempotency-Key must be 8-128 characters of [A-Za-z0-9_-]")]
    InvalidKey,
    /// The same request is still running.
    #[error("a request with this Idempotency-Key is still in progress")]
    InFlight,
    /// The registry lock was poisoned.
    #[error("idempotency registry unavailable")]
    Poisoned,
}

/// Outcome of claiming a key.
#[derive(Debug)]
pub(crate) enum Claim {
    /// First time: run the request, then [`SlotGuard::finish`] it.
    Fresh(SlotGuard),
    /// Already done: return this body without re-running.
    Replay(serde_json::Value),
}

impl Idempotency {
    /// Claims `key` within `scope` (route + session), so one key cannot
    /// replay another endpoint's result.
    ///
    /// # Errors
    /// Invalid key, a concurrent duplicate, or a poisoned lock.
    pub(crate) fn claim(&self, scope: &str, key: &str) -> Result<Claim, ClaimError> {
        if !KEY_LEN.contains(&key.len())
            || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ClaimError::InvalidKey);
        }
        let slot = format!("{scope}\u{1f}{key}");
        let now = Instant::now();
        let mut entries = self.entries.lock().map_err(|_poisoned| ClaimError::Poisoned)?;
        prune(&mut entries, now);
        match entries.get(&slot) {
            Some(Entry::Done { body, .. }) => return Ok(Claim::Replay(body.clone())),
            Some(Entry::InFlight { .. }) => return Err(ClaimError::InFlight),
            None => {}
        }
        entries.insert(slot.clone(), Entry::InFlight { started: now });
        Ok(Claim::Fresh(SlotGuard { registry: self.clone(), slot, finished: false }))
    }

    fn complete(&self, slot: &str, body: Option<&serde_json::Value>) {
        let Ok(mut entries) = self.entries.lock() else {
            tracing::error!("idempotency registry poisoned");
            return;
        };
        match body {
            Some(body) => {
                entries.insert(
                    slot.to_owned(),
                    Entry::Done { finished: Instant::now(), body: body.clone() },
                );
            }
            None => {
                entries.remove(slot);
            }
        }
    }
}

/// A claimed key. Dropping it unfinished (handler cancelled because the
/// client disconnected) forgets the key so the retry can run.
#[derive(Debug)]
pub(crate) struct SlotGuard {
    registry: Idempotency,
    slot: String,
    finished: bool,
}

impl SlotGuard {
    /// Stores a success body for replay, or forgets the key on failure.
    pub(crate) fn finish(mut self, body: Option<&serde_json::Value>) {
        self.finished = true;
        self.registry.complete(&self.slot, body);
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.registry.complete(&self.slot, None);
        }
    }
}

fn prune(entries: &mut HashMap<String, Entry>, now: Instant) {
    entries.retain(|_, entry| now.saturating_duration_since(entry.age_anchor()) < RESULT_TTL);
    while entries.len() >= MAX_ENTRIES {
        let Some(oldest) =
            entries.iter().min_by_key(|(_, entry)| entry.age_anchor()).map(|(key, _)| key.clone())
        else {
            break;
        };
        entries.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use super::{Claim, ClaimError, Idempotency};

    #[test]
    fn first_claim_runs_and_retry_replays_the_result() {
        let registry = Idempotency::default();
        let Ok(Claim::Fresh(guard)) = registry.claim("message:s1", "key-00000001") else {
            panic!("first claim must be fresh");
        };
        guard.finish(Some(&serde_json::json!({"accepted": true})));
        let replay = registry.claim("message:s1", "key-00000001");
        assert!(
            matches!(replay, Ok(Claim::Replay(ref body)) if body["accepted"] == true),
            "retry returns the stored body"
        );
    }

    #[test]
    fn concurrent_duplicate_is_rejected_while_in_flight() {
        let registry = Idempotency::default();
        let first = registry.claim("message:s1", "key-00000002");
        assert!(matches!(first, Ok(Claim::Fresh(_))), "first claim is fresh");
        // `first` (and its guard) stays alive: the request is still running.
        assert_eq!(
            registry.claim("message:s1", "key-00000002").err(),
            Some(ClaimError::InFlight),
            "duplicate while running is refused"
        );
    }

    #[test]
    fn failure_forgets_the_key_so_retry_can_run() {
        let registry = Idempotency::default();
        let Ok(Claim::Fresh(guard)) = registry.claim("respond:s1", "key-00000003") else {
            panic!("first claim must be fresh");
        };
        guard.finish(None);
        assert!(
            matches!(registry.claim("respond:s1", "key-00000003"), Ok(Claim::Fresh(_))),
            "a failed attempt may be retried"
        );
    }

    #[test]
    fn cancelled_request_releases_the_key() {
        let registry = Idempotency::default();
        let claim = registry.claim("message:s1", "key-00000005");
        assert!(matches!(claim, Ok(Claim::Fresh(_))), "first claim is fresh");
        drop(claim);
        assert!(
            matches!(registry.claim("message:s1", "key-00000005"), Ok(Claim::Fresh(_))),
            "a dropped (cancelled) handler must not block the retry"
        );
    }

    #[test]
    fn keys_are_scoped_per_route_and_session() {
        let registry = Idempotency::default();
        let Ok(Claim::Fresh(guard)) = registry.claim("message:s1", "key-00000004") else {
            panic!("first claim must be fresh");
        };
        guard.finish(Some(&serde_json::json!({"accepted": true})));
        assert!(
            matches!(registry.claim("message:s2", "key-00000004"), Ok(Claim::Fresh(_))),
            "same key on another session is independent"
        );
    }

    #[test]
    fn rejects_malformed_keys() {
        let registry = Idempotency::default();
        for key in ["short", "has space in it", "semi;colon;key", &"x".repeat(129)] {
            assert_eq!(
                registry.claim("message:s1", key).err(),
                Some(ClaimError::InvalidKey),
                "{key:?} must be rejected"
            );
        }
    }
}
