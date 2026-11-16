//! Bounded retries for complete, rollback-safe commercial transactions.
use std::future::Future;
use talocart_platform::technical::TechnicalError;

/// The operation owns its transaction and must not perform external side effects.
/// Preserve the final error; only database serialization/deadlock causes retry.
pub(crate) async fn run<T, E, F, Fut>(
    name: &'static str,
    mut operation: F,
    technical: fn(&E) -> Option<TechnicalError>,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut retries = 0;
    loop {
        let result = operation().await;
        if retries < 2
            && result
                .as_ref()
                .err()
                .and_then(technical)
                .is_some_and(|e| e.retryable())
        {
            retries += 1;
            tracing::warn!(target: "talocart::transactions", operation = name, attempt = retries, "Retrying transaction conflict");
        } else {
            return result;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use talocart_platform::technical::DatabaseFailure;

    #[tokio::test]
    async fn non_transaction_failures_are_never_retried() {
        for error in [
            TechnicalError::Busy,
            TechnicalError::Storage,
            TechnicalError::Unavailable,
            DatabaseFailure::capture(sqlx::Error::PoolClosed, module_path!()).into(),
        ] {
            let mut attempts = 0;
            let result: Result<(), _> = run(
                "test",
                || {
                    attempts += 1;
                    std::future::ready(Err(error.clone()))
                },
                |e| Some(e.clone()),
            )
            .await;
            assert_eq!(result, Err(error));
            assert_eq!(attempts, 1);
        }
    }
}
