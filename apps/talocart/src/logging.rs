//! JSON console diagnostics and a bounded, off-thread private error journal.
use std::{fs, path::Path};
use talocart_platform::config::LoggingConfig;
use tracing_appender::{
    non_blocking::{ErrorCounter, WorkerGuard},
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{EnvFilter, filter::FilterExt, prelude::*};

pub struct LoggingGuard {
    worker: Option<WorkerGuard>,
    counter: ErrorCounter,
}
impl Drop for LoggingGuard {
    fn drop(&mut self) {
        drop(self.worker.take());
        let dropped = self.counter.dropped_lines();
        if dropped > 0 {
            eprintln!(
                "Error journal: {dropped} records dropped because the logging queue was full"
            );
        }
    }
}

// log::log_enabled! probes (including PostgreSQL notices) can leave stale
// per-layer state: https://github.com/tokio-rs/tracing/issues/2927.
// Recheck accepted metadata while retaining each filter's span callbacks.
fn rechecked<S, F>(filter: F) -> impl tracing_subscriber::layer::Filter<S>
where
    S: tracing::Subscriber,
    F: tracing_subscriber::layer::Filter<S>,
{
    filter.and(tracing_subscriber::filter::dynamic_filter_fn(|_, _| true))
}

pub fn initialize(
    filter: EnvFilter,
    config: Option<&LoggingConfig>,
) -> Result<Option<LoggingGuard>, &'static str> {
    let console = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(std::io::stderr)
        .with_filter(rechecked(filter));
    let (file, guard) = if let Some(config) = config {
        let (writer, guard) = journal(config)?;
        let counter = writer.error_counter();
        let layer = tracing_subscriber::fmt::layer()
            .json()
            .with_ansi(false)
            .with_writer(writer)
            // Preserve HTTP span context even though only ERROR events are persisted.
            .with_filter(rechecked(tracing_subscriber::filter::filter_fn(
                journal_metadata,
            )));
        (
            Some(layer),
            Some(LoggingGuard {
                worker: Some(guard),
                counter,
            }),
        )
    } else {
        (None, None)
    };
    tracing_subscriber::registry()
        .with(console)
        .with(file)
        .try_init()
        .map_err(|_| "Unable to initialize diagnostics")?;
    Ok(guard)
}

fn journal_metadata(meta: &tracing::Metadata<'_>) -> bool {
    (meta.is_span() && meta.name() == "http" && meta.target() == "talocart_web::routing")
        || (meta.is_event() && *meta.level() == tracing::Level::ERROR)
}

fn journal(
    config: &LoggingConfig,
) -> Result<(tracing_appender::non_blocking::NonBlocking, WorkerGuard), &'static str> {
    private_directory(&config.root).map_err(|_| "Unable to prepare the private log directory")?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("errors")
        .filename_suffix("jsonl")
        .max_log_files(config.max_files)
        .build(&config.root)
        .map_err(|_| "Unable to open the error journal")?;
    let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(4096)
        .finish(ReportingWriter {
            inner: appender,
            failed: false,
        });
    Ok((writer, guard))
}

// Appender's worker ignores I/O errors; report a sanitized transition on stderr.
struct ReportingWriter<W> {
    inner: W,
    failed: bool,
}
impl<W: std::io::Write> ReportingWriter<W> {
    fn report<T>(&mut self, result: std::io::Result<T>) -> std::io::Result<T> {
        if let Err(error) = &result {
            if !self.failed {
                eprintln!("Error journal write failed: {:?}", error.kind());
            }
            self.failed = true;
        } else {
            self.failed = false;
        }
        result
    }
}
impl<W: std::io::Write> std::io::Write for ReportingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let result = self.inner.write(bytes);
        self.report(result)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let result = self.inner.flush();
        self.report(result)
    }
}

fn private_directory(root: &Path) -> std::io::Result<()> {
    fs::create_dir_all(root)?;
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::other("Invalid log directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    // Reject links and special files before the appender opens any existing journal.
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("errors.")
            && !entry.file_type()?.is_file()
        {
            return Err(std::io::Error::other("Invalid log file"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn suppressed_library_probe_does_not_hide_the_next_error() {
        use tracing::callsite::Callsite;
        for console_level in ["warn", "off"] {
            let root = std::env::temp_dir().join(format!(
                "talocart-log-probe-{}",
                talocart_platform::identity::token().unwrap()
            ));
            let config = LoggingConfig {
                root: root.clone(),
                max_files: 2,
            };
            let (writer, guard) = journal(&config).unwrap();
            let console = std::sync::Arc::new(fs::File::create(root.join("console.log")).unwrap());
            let subscriber = tracing_subscriber::registry()
                .with(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_writer(console.clone())
                        .with_filter(rechecked(EnvFilter::new(console_level))),
                )
                .with(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_writer(writer)
                        .with_filter(rechecked(tracing_subscriber::filter::filter_fn(
                            journal_metadata,
                        ))),
                );
            tracing::subscriber::with_default(subscriber, || {
                // The log bridge calls Dispatch::enabled directly for PostgreSQL
                // notices; it may not emit the probed event afterwards.
                let probe = tracing::callsite! {
                    name: "suppressed library probe",
                    kind: tracing::metadata::Kind::EVENT,
                    target: "sqlx::query",
                    level: tracing::Level::INFO,
                    fields: message
                };
                tracing::dispatcher::get_default(|dispatch| {
                    let _ = dispatch.enabled(probe.metadata());
                });
                tracing::error!("First sanitized error after probe");
            });
            drop(guard);
            let text = fs::read_dir(&root)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("errors.")
                })
                .map(|p| fs::read_to_string(p).unwrap())
                .collect::<String>();
            let console = fs::read_to_string(root.join("console.log")).unwrap();
            fs::remove_dir_all(root).unwrap();
            assert_eq!(
                console.contains("First sanitized error after probe"),
                console_level != "off"
            );
            assert!(text.contains("First sanitized error after probe"));
        }
    }

    #[test]
    fn journal_excludes_library_span_fields_even_with_verbose_console() {
        let root = std::env::temp_dir().join(format!(
            "talocart-log-privacy-{}",
            talocart_platform::identity::token().unwrap()
        ));
        let config = LoggingConfig {
            root: root.clone(),
            max_files: 2,
        };
        let (writer, guard) = journal(&config).unwrap();
        let subscriber = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(std::io::sink)
                    .with_filter(rechecked(EnvFilter::new("trace"))),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(writer)
                    .with_filter(rechecked(tracing_subscriber::filter::filter_fn(
                        journal_metadata,
                    ))),
            );
        tracing::subscriber::with_default(subscriber, || {
            let request = tracing::info_span!(target: "talocart_web::routing", "http", request_id = "safe-request");
            let _request = request.enter();
            let query = tracing::debug_span!(target: "sqlx::query", "query", statement = "PRIVATE_SQL_AND_PARAMETERS");
            let _query = query.enter();
            tracing::error!("Sanitized failure");
        });
        drop(guard);
        let text = fs::read_dir(&root)
            .unwrap()
            .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
            .collect::<String>();
        fs::remove_dir_all(root).unwrap();
        assert!(text.contains("safe-request"));
        assert!(text.contains("Sanitized failure"));
        assert!(!text.contains("PRIVATE_SQL_AND_PARAMETERS"));
    }

    #[test]
    fn journal_filters_events_keeps_context_appends_and_limits_files() {
        let root = std::env::temp_dir().join(format!(
            "talocart-writer-{}",
            talocart_platform::identity::token().unwrap()
        ));
        fs::create_dir_all(&root).unwrap();
        for day in 1..=4 {
            fs::write(root.join(format!("errors.2020-01-{day:02}.jsonl")), "old\n").unwrap();
        }
        fs::write(root.join("unrelated.txt"), "keep").unwrap();
        let config = LoggingConfig {
            root: root.clone(),
            max_files: 2,
        };
        for n in 0..2 {
            let (writer, guard) = journal(&config).unwrap();
            let subscriber = tracing_subscriber::registry().with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(writer)
                    .with_filter(rechecked(tracing_subscriber::filter::filter_fn(
                        journal_metadata,
                    ))),
            );
            tracing::subscriber::with_default(subscriber, || {
                let span = tracing::info_span!(target: "talocart_web::routing", "http", request_id = "synthetic-request");
                let _entered = span.enter();
                tracing::info!("excluded");
                tracing::error!(attempt = n, "synthetic failure");
            });
            drop(guard);
        }
        let files: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        assert!(files.len() <= 2);
        let text = files
            .iter()
            .map(|p| fs::read_to_string(p).unwrap())
            .collect::<String>();
        assert_eq!(text.matches("synthetic failure").count(), 2);
        assert!(text.contains("synthetic-request"));
        assert!(!text.contains("excluded"));
        assert!(root.join("unrelated.txt").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            std::os::unix::fs::symlink(root.join("unrelated.txt"), root.join("errors.link.jsonl"))
                .unwrap();
            assert!(journal(&config).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
