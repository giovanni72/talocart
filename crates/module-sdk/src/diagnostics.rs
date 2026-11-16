//! Private, bounded asynchronous diagnostics. Arbitrary tracing field values are discarded.
use std::{
    fs,
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use tracing_appender::{
    non_blocking::{NonBlocking, WorkerGuard},
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

pub struct Diagnostics {
    writer: Option<NonBlocking>,
    guard: Option<WorkerGuard>,
    phase: &'static str,
    completed: bool,
}
impl Diagnostics {
    pub async fn open(
        data_root: &Path,
        action: talocart_platform::packages::protocol::Action,
    ) -> super::Result<Self> {
        if action == talocart_platform::packages::protocol::Action::CanStop {
            return Ok(Self {
                writer: None,
                guard: None,
                phase: "read-only probe",
                completed: true,
            });
        }
        let root = data_root.join("logs");
        tokio::task::spawn_blocking(move || {
            fs::create_dir_all(&root)?;
            let meta = fs::symlink_metadata(&root)?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err("Invalid module log directory".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            }
            for entry in fs::read_dir(&root)? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    return Err("Invalid module log file".into());
                }
            }
            let appender = RollingFileAppender::builder()
                .rotation(Rotation::DAILY)
                .filename_prefix("module")
                .filename_suffix("jsonl")
                .max_log_files(7)
                .build(root)?;
            let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
                .buffered_lines_limit(256)
                .finish(appender);
            tracing_subscriber::registry()
                .with(MetadataOnly(writer.clone()))
                .try_init()?;
            let mut diagnostics = Self {
                writer: Some(writer),
                guard: Some(guard),
                phase: "starting",
                completed: false,
            };
            diagnostics.record("INFO", "Module operation started");
            Ok(diagnostics)
        })
        .await?
    }
    pub fn phase(&mut self, phase: &'static str) {
        self.phase = phase;
    }
    pub fn complete(mut self) {
        self.completed = true;
        self.record("INFO", "Module operation completed");
    }
    fn record(&mut self, level: &str, message: &str) {
        let value = serde_json::json!({"timestamp":timestamp(),"level":level,"phase":self.phase,"message":message});
        if let Some(writer) = &mut self.writer {
            let _ = writeln!(writer, "{value}");
        }
    }
}
impl Drop for Diagnostics {
    fn drop(&mut self) {
        if !self.completed {
            self.record("ERROR", "Module operation failed");
        }
        // Flush after recording the terminal outcome, including early errors.
        drop(self.guard.take());
    }
}
fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}
struct MetadataOnly(NonBlocking);
impl<S: tracing::Subscriber> Layer<S> for MetadataOnly {
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() > tracing::Level::WARN {
            return;
        }
        // No SQL, HTTP URLs, credentials, exception text, spans or user-supplied fields.
        let value = metadata_event(meta);
        let _ = writeln!(self.0.clone(), "{value}");
    }
}
fn metadata_event(meta: &tracing::Metadata<'_>) -> serde_json::Value {
    serde_json::json!({"timestamp":timestamp(),"level":meta.level().as_str(),"target":meta.target(),"module":meta.module_path(),"line":meta.line(),"message":"Module diagnostic; inspect the recorded source location"})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn offline_probe_never_creates_or_opens_logs() {
        let root = std::env::temp_dir().join(format!(
            "module-read-only-{}",
            talocart_platform::identity::token().unwrap()
        ));
        Diagnostics::open(
            &root,
            talocart_platform::packages::protocol::Action::CanStop,
        )
        .await
        .unwrap()
        .complete();
        assert!(!root.exists());
    }

    #[test]
    fn tracing_values_never_enter_the_private_journal() {
        struct Capture(std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>);
        impl<S: tracing::Subscriber> Layer<S> for Capture {
            fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
                self.0
                    .lock()
                    .unwrap()
                    .push(metadata_event(event.metadata()));
            }
        }
        let events = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(Capture(events.clone())),
            || {
                tracing::warn!(
                    password = "private-password",
                    error = "postgres://private-secret",
                    "private-user-message"
                );
            },
        );
        let text = serde_json::to_string(&*events.lock().unwrap()).unwrap();
        assert!(text.contains("WARN"));
        assert!(!text.contains("private-"));
    }
}
