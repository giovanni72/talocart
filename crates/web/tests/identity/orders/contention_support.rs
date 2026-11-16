// Controlled database conflicts, without using elapsed time as evidence of overlap.
use super::*;
use std::time::Duration;
use talocart_platform::database::commercial_lock;



async fn wait_for_blocked(pool: &PgPool, blocker: i32, expected: i64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let blocked: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))",
            )
            .bind(blocker)
            .fetch_one(pool)
            .await
            .unwrap();
            if blocked >= expected {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Confirmations must reach the controlled database conflict");
}
