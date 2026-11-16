use super::*;
use lettre::AsyncTransport;
use std::time::Duration;
impl Mail {
    /// At most one message per call, with a committed two-minute lease. Safe across processes.
    /// A lost SMTP acknowledgement/recovered lease may resend the same stable Message-ID.
    pub async fn process_one(&self, config: &SmtpConfig) -> Result<bool> {
        if !config.enabled {
            return Ok(false);
        }
        let smtp = config.transport()?;
        let Some(job) = self.claim().await? else {
            return Ok(false);
        };
        let outcome = match job.message() {
            Err(_) => "invalid",
            Ok(message) => match tokio::time::timeout(
                Duration::from_secs(config.timeout_seconds),
                smtp.send(message),
            )
            .await
            {
                Ok(Ok(_)) => "accepted",
                Ok(Err(e)) if e.is_timeout() => "timeout",
                Ok(Err(e)) if e.is_permanent() || e.is_client() => "permanent",
                Ok(Err(_)) => "transient",
                Err(_) => "timeout",
            },
        };
        self.finish(&job, outcome).await?;
        Ok(true)
    }
    async fn claim(&self) -> Result<Option<Claimed>> {
        let mut tx = self.pool.begin().await?;
        let enabled: bool =
            sqlx::query_scalar("SELECT enabled FROM email_configuration WHERE id=1 FOR SHARE")
                .fetch_one(&mut *tx)
                .await?;
        if !enabled {
            return Ok(None);
        }
        // Recover bounded batches. A crashed sender's result is unknown, never called delivered.
        let expired:Vec<(i64,i32)>=sqlx::query_as("SELECT id,attempts FROM email_outbox WHERE state='sending' AND lease_until<clock_timestamp() ORDER BY lease_until,id FOR UPDATE SKIP LOCKED LIMIT 20").fetch_all(&mut *tx).await?;
        for (id, attempt) in expired {
            sqlx::query("UPDATE email_attempts SET outcome='interrupted',finished_at=clock_timestamp() WHERE message_id=$1 AND attempt=$2 AND outcome='sending'").bind(id).bind(attempt).execute(&mut *tx).await?;
            sqlx::query("UPDATE email_outbox SET state=CASE WHEN superseded THEN 'superseded' WHEN cycle_attempts>=5 THEN 'failed' ELSE 'pending' END,lease_until=NULL,lease_token=NULL,available_at=clock_timestamp()+interval '30 seconds',last_outcome='interrupted' WHERE id=$1").bind(id).execute(&mut *tx).await?;
        }
        let job:Option<Claimed>=sqlx::query_as("UPDATE email_outbox SET state='sending',attempts=attempts+1,cycle_attempts=cycle_attempts+1,lease_token=$1,lease_until=clock_timestamp()+interval '2 minutes' WHERE id=(SELECT e.id FROM email_outbox e WHERE e.state='pending' AND NOT e.superseded AND e.available_at<=clock_timestamp() AND e.cycle_attempts<5 AND NOT EXISTS(SELECT 1 FROM email_outbox prior WHERE prior.order_id=e.order_id AND (prior.template IN ('order_received','order_updated','order_admin') OR (e.template='payment_refunded' AND prior.template='payment_received') OR (e.template IN ('order_shipped','order_tracking','order_delivered') AND prior.template IN ('order_shipped','order_tracking','order_delivered'))) AND prior.state='sending' AND e.template IN ('order_updated','order_cancelled','cancellation_admin','payment_received','payment_refunded','order_shipped','order_tracking','order_delivered')) ORDER BY e.available_at,e.id FOR UPDATE OF e SKIP LOCKED LIMIT 1) RETURNING id,recipient,subject,body_text,body_html,from_name,from_email,reply_to,message_id,lease_token,attempts,cycle_attempts")
            .bind(token()?).fetch_optional(&mut *tx).await?;
        if let Some(j) = &job {
            sqlx::query(
                "INSERT INTO email_attempts(message_id,attempt,outcome) VALUES($1,$2,'sending')",
            )
            .bind(j.id)
            .bind(j.attempts)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(job)
    }
    async fn finish(&self, job: &Claimed, outcome: &str) -> Result<()> {
        let state = match outcome {
            "accepted" => "accepted",
            "invalid" | "permanent" => "failed",
            _ if job.cycle_attempts >= 5 => "failed",
            _ => "pending",
        };
        let backoff = 30i32 * 2i32.pow((job.cycle_attempts - 1) as u32);
        let mut tx = self.pool.begin().await?;
        let changed=sqlx::query("UPDATE email_outbox SET state=CASE WHEN superseded AND $3<>'accepted' THEN 'superseded' ELSE $3 END,lease_until=NULL,lease_token=NULL,last_outcome=$4,accepted_at=CASE WHEN $3='accepted' THEN clock_timestamp() ELSE NULL END,available_at=clock_timestamp()+make_interval(secs=>$5) WHERE id=$1 AND state='sending' AND lease_token=$2 AND lease_until>clock_timestamp()")
            .bind(job.id).bind(&job.lease_token).bind(state).bind(outcome).bind(f64::from(backoff)).execute(&mut *tx).await?.rows_affected();
        if changed == 1 {
            sqlx::query("UPDATE email_attempts SET outcome=$3,finished_at=clock_timestamp() WHERE message_id=$1 AND attempt=$2 AND outcome='sending'").bind(job.id).bind(job.attempts).bind(outcome).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_header_injection_and_non_addresses() {
        for value in [
            "name <a@example.test>",
            "a@example.test,b@example.test",
            "a@example.test\r\nBcc:x@example.test",
            "bad",
        ] {
            assert!(address(value).is_err(), "{value:?}");
        }
        assert_eq!(
            address(" OWNER@EXAMPLE.TEST ").unwrap(),
            "owner@example.test"
        );
    }
}
