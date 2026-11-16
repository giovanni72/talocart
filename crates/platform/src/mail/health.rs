//! Outbox diagnostics expose counts and due age, never recipients or message content.
use super::*;
impl Mail {
    pub async fn queue_health(&self) -> Result<crate::work::QueueHealth> {
        Ok(sqlx::query_as(
            "SELECT count(*) FILTER (WHERE state='pending' AND NOT superseded) AS pending,
             count(*) FILTER (WHERE state='sending') AS running,
             count(*) FILTER (WHERE state='failed') AS failed,
             count(*) FILTER (WHERE state='pending' AND NOT superseded AND attempts>0) AS retrying,
             count(*) FILTER (WHERE state='sending' AND lease_until<clock_timestamp()) AS expired_leases,
             coalesce(max(extract(epoch FROM clock_timestamp()-available_at)) FILTER
               (WHERE state='pending' AND NOT superseded AND available_at<=clock_timestamp()),0)::bigint AS oldest_due_seconds
             FROM email_outbox WHERE state IN ('pending','sending','failed')",
        ).fetch_one(&self.pool).await?)
    }
}
