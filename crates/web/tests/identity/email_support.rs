use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicUsize},
};
use talocart_commerce::notifications::{self, Notice};
use talocart_platform::mail::{Mail, MailError, Settings, SmtpConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn content() -> talocart_platform::mail::Content {
    notifications::render(
        &notifications::sample().unwrap(),
        Notice::OrderReceived,
        "it",
        true,
    )
    .unwrap()
}
async fn configured(pool: &PgPool) -> (Identity, talocart_platform::identity::Session, Mail) {
    let identity = installed(pool).await;
    let s = taxonomy_session(&identity).await;
    let mail = Mail::new(pool.clone(), identity.clone());
    mail.save(
        &s,
        &s.csrf,
        &Settings {
            revision: 0,
            enabled: true,
            from_name: "Test shop".into(),
            from_email: "sender@example.test".into(),
            reply_to: "support@example.test".into(),
            admin_recipients: vec!["admin@example.test".into()],
        },
    )
    .await
    .unwrap();
    (identity, s, mail)
}
async fn enqueue(pool: &PgPool, key: &str) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    let id = Mail::enqueue(&mut tx, key, "customer@example.test", &content())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}
async fn state(pool: &PgPool, id: i64) -> (String, i32, i16) {
    sqlx::query_as("SELECT state,attempts,cycle_attempts FROM email_outbox WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}
// SMTP fixture for deterministic errors. Real MIME/capture is independently tested with Mailpit.
pub(super) async fn smtp(
    mode: i32,
) -> (
    SmtpConfig,
    Arc<AtomicI32>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = SmtpConfig {
        enabled: true,
        port: listener.local_addr().unwrap().port(),
        timeout_seconds: 1,
        ..Default::default()
    };
    let mode = Arc::new(AtomicI32::new(mode));
    let count = Arc::new(AtomicUsize::new(0));
    let m = mode.clone();
    let c = count.clone();
    let task = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let m = m.clone();
            let c = c.clone();
            tokio::spawn(async move {
                let (reader, mut writer) = socket.into_split();
                let mut lines = BufReader::new(reader).lines();
                if writer.write_all(b"220 test SMTP\r\n").await.is_err() {
                    return;
                }
                while let Ok(Some(line)) = lines.next_line().await {
                    let reply = if line.starts_with("EHLO") {
                        "250-test\r\n250 8BITMIME\r\n"
                    } else if line.starts_with("MAIL FROM") {
                        match m.load(Ordering::SeqCst) {
                            451 => "451 temporary\r\n",
                            550 => "550 permanent\r\n",
                            _ => "250 OK\r\n",
                        }
                    } else if line == "DATA" {
                        if writer.write_all(b"354 continue\r\n").await.is_err() {
                            return;
                        }
                        while let Ok(Some(part)) = lines.next_line().await {
                            if part == "." {
                                break;
                            }
                        }
                        c.fetch_add(1, Ordering::SeqCst);
                        if m.load(Ordering::SeqCst) == 999 {
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        }
                        "250 accepted\r\n"
                    } else if line == "QUIT" {
                        let _ = writer.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        "250 OK\r\n"
                    };
                    if writer.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (config, mode, count, task)
}











// Regressions from the email audit.
