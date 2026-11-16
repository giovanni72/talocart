//! Provider queue diagnostics. Scheduled paid-order polling is not a backlog until due.
use super::*;
impl OnlinePayments {
    /// Read-only and independent of provider configuration or network availability.
    pub async fn queue_health(pool: &PgPool) -> Result<talocart_platform::work::QueueHealth> {
        Ok(sqlx::query_as(
            "SELECT count(*) FILTER (WHERE action<>'none' AND lease_token IS NULL) AS pending,
             count(*) FILTER (WHERE lease_token IS NOT NULL) AS running,
             count(*) FILTER (WHERE state='review') AS failed,
             count(*) FILTER (WHERE action<>'none' AND lease_token IS NULL AND error_count>0) AS retrying,
             count(*) FILTER (WHERE lease_until<clock_timestamp()) AS expired_leases,
             coalesce(max(extract(epoch FROM clock_timestamp()-next_at)) FILTER
               (WHERE action<>'none' AND lease_token IS NULL AND next_at<=clock_timestamp()),0)::bigint AS oldest_due_seconds
             FROM payment_attempts WHERE action<>'none' OR state='review' OR lease_token IS NOT NULL",
        ).fetch_one(pool).await?)
    }
}
