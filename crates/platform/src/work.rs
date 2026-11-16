//! Read-only operational summaries. Each queue owner supplies its own query.
#[derive(Clone, Debug, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct QueueHealth {
    pub pending: i64,
    pub running: i64,
    pub failed: i64,
    pub retrying: i64,
    pub expired_leases: i64,
    /// Age since a pending job became due, not the age of future scheduled work.
    pub oldest_due_seconds: i64,
}
