//! Periodic bounded operational logging. No public endpoint or provider calls.
use std::time::Duration;
use talocart_platform::{database::PgPool, identity::Identity, mail::Mail, work::QueueHealth};

fn emit(queue: &'static str, health: QueueHealth, pool: &PgPool) {
    tracing::info!(target:"talocart::workers", queue,
        pending=health.pending,running=health.running,failed=health.failed,
        retrying=health.retrying,expired_leases=health.expired_leases,
        oldest_due_seconds=health.oldest_due_seconds,
        pool_size=pool.size(),pool_idle=pool.num_idle(),"Persistent queue health");
}

pub(super) fn spawn(pool: PgPool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mail = Mail::new(pool.clone(), Identity::new(pool.clone()));
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            // One sample at a time; unavailable storage must not prevent serving/shutdown.
            let sample = async {
                let email = mail.queue_health().await.map_err(|_| ())?;
                let payments = talocart_commerce::orders::OnlinePayments::queue_health(&pool)
                    .await
                    .map_err(|_| ())?;
                emit("email", email, &pool);
                emit("payments", payments, &pool);
                Ok::<_, ()>(())
            };
            if !matches!(
                tokio::time::timeout(Duration::from_secs(2), sample).await,
                Ok(Ok(()))
            ) {
                tracing::warn!(target:"talocart::workers", "Queue diagnostics temporarily unavailable");
            }
        }
    })
}
