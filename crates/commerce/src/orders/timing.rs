//! One safe completion event per confirmation attempt, including early errors/cancellation.
use std::time::Instant;
pub(super) struct ConfirmationTiming {
    started: Instant,
    acquired: Option<Instant>,
    locked: Option<Instant>,
    pub(super) outcome: &'static str,
}
impl ConfirmationTiming {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            acquired: None,
            locked: None,
            outcome: "aborted",
        }
    }
    pub(super) fn acquired(&mut self) {
        self.acquired = Some(Instant::now());
    }
    pub(super) fn locked(&mut self) {
        self.locked = Some(Instant::now());
    }
}
impl Drop for ConfirmationTiming {
    fn drop(&mut self) {
        let finished = Instant::now();
        // Microseconds are encoded as u64; saturation affects only diagnostic durations.
        let micros =
            |duration: std::time::Duration| u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        tracing::info!(target: "talocart::checkout",
            operation="confirmation",outcome=self.outcome,
            total_us=micros(finished.duration_since(self.started)),
            pool_wait_us=micros(self.acquired.unwrap_or(finished).duration_since(self.started)),
            lock_wait_us=micros(match (self.acquired,self.locked) {
                (Some(acquired),locked)=>locked.unwrap_or(finished).duration_since(acquired),
                _=>std::time::Duration::ZERO,
            }),
            transaction_us=micros(self.acquired.map(|acquired|finished.duration_since(acquired)).unwrap_or_default()),
            pool_acquired=self.acquired.is_some(),commercial_lock_acquired=self.locked.is_some(),
            "Order confirmation attempt finished");
    }
}
