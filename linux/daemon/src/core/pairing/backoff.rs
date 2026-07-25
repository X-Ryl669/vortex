//! Reconnect backoff state per spec §7.7.
//!
//! ```text
//! | Attempt # | Delay before this attempt |
//! |-----------|---------------------------|
//! | 1         | 0s (immediate)            |
//! | 2         | 5s                        |
//! | 3         | 15s                       |
//! | 4         | 1m                        |
//! | 5         | 5m                        |
//! | 6+        | passive only              |
//! ```

use std::time::Duration;

/// V1 production backoff schedule. The trailing `None` means "passive only".
pub const DEFAULT_SCHEDULE: &[Option<Duration>] = &[
    Some(Duration::from_secs(0)),
    Some(Duration::from_secs(5)),
    Some(Duration::from_secs(15)),
    Some(Duration::from_secs(60)),
    Some(Duration::from_secs(5 * 60)),
    None,
];

/// Test schedule with millisecond delays — same shape, drastically shorter.
/// Used by `--fast` and unit tests so behaviour can be validated quickly.
pub const FAST_SCHEDULE: &[Option<Duration>] = &[
    Some(Duration::from_millis(0)),
    Some(Duration::from_millis(50)),
    Some(Duration::from_millis(150)),
    Some(Duration::from_millis(600)),
    Some(Duration::from_millis(3_000)),
    None,
];

/// A per-peer backoff cursor. Cheap to clone; not thread-safe (caller
/// owns synchronisation).
#[derive(Debug, Clone)]
pub struct BackoffState {
    schedule: &'static [Option<Duration>],
    /// 0-based index into `schedule`. After clamping at `schedule.len() - 1`
    /// further failures keep the state in passive mode.
    cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextAction {
    /// Sleep for [`Duration`] and then attempt the operation.
    AttemptAfter(Duration),
    /// Schedule says passive only — wait for a reset trigger.
    Passive,
}

impl BackoffState {
    pub fn new(schedule: &'static [Option<Duration>]) -> Self {
        assert!(
            !schedule.is_empty(),
            "backoff schedule must have at least one entry"
        );
        Self { schedule, cursor: 0 }
    }

    pub fn standard() -> Self {
        Self::new(DEFAULT_SCHEDULE)
    }

    pub fn fast() -> Self {
        Self::new(FAST_SCHEDULE)
    }

    /// What to do for the *current* attempt.
    pub fn next_action(&self) -> NextAction {
        let i = self.cursor.min(self.schedule.len() - 1);
        match self.schedule[i] {
            Some(d) => NextAction::AttemptAfter(d),
            None => NextAction::Passive,
        }
    }

    /// True iff the state has reached passive-only mode.
    pub fn is_passive(&self) -> bool {
        matches!(self.next_action(), NextAction::Passive)
    }

    /// Human-readable attempt counter for logs (1-based).
    pub fn attempt_number(&self) -> usize {
        self.cursor + 1
    }

    /// Record a failed attempt; advance one step (clamped at the passive
    /// terminus).
    pub fn record_failure(&mut self) {
        if self.cursor + 1 < self.schedule.len() {
            self.cursor += 1;
        } else {
            self.cursor = self.schedule.len() - 1;
        }
    }

    /// Reset to attempt #1. Call on user-triggered retry, network change,
    /// adapter on, app foreground, or fresh trusted-presence (§7.7.1).
    pub fn reset(&mut self) {
        self.cursor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_attempt_is_immediate() {
        let s = BackoffState::standard();
        assert_eq!(s.next_action(), NextAction::AttemptAfter(Duration::from_secs(0)));
        assert_eq!(s.attempt_number(), 1);
        assert!(!s.is_passive());
    }

    #[test]
    fn schedule_progression() {
        let mut s = BackoffState::standard();
        let expected = [0, 5, 15, 60, 300];
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(s.attempt_number(), i + 1);
            assert_eq!(
                s.next_action(),
                NextAction::AttemptAfter(Duration::from_secs(*want)),
                "attempt {}",
                i + 1
            );
            s.record_failure();
        }
        assert!(s.is_passive(), "after the last delayed attempt, must be passive");
    }

    #[test]
    fn passive_persists_across_failures() {
        let mut s = BackoffState::standard();
        for _ in 0..10 {
            s.record_failure();
        }
        assert!(s.is_passive());
        s.record_failure();
        assert!(s.is_passive());
    }

    #[test]
    fn reset_returns_to_first_attempt() {
        let mut s = BackoffState::standard();
        for _ in 0..10 {
            s.record_failure();
        }
        assert!(s.is_passive());
        s.reset();
        assert_eq!(s.next_action(), NextAction::AttemptAfter(Duration::from_secs(0)));
        assert_eq!(s.attempt_number(), 1);
    }

    #[test]
    fn fast_schedule_shape_matches_default() {
        assert_eq!(DEFAULT_SCHEDULE.len(), FAST_SCHEDULE.len());
        for i in 0..DEFAULT_SCHEDULE.len() {
            assert_eq!(
                DEFAULT_SCHEDULE[i].is_some(),
                FAST_SCHEDULE[i].is_some(),
                "passive position differs at index {i}",
            );
        }
    }
}
