//! Own every task until shutdown; an unexpected exit stops the host.
use std::{fmt, future::Future};
use tokio::{
    sync::watch,
    task::{AbortHandle, JoinHandle, JoinSet},
    time::Instant,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Error {
    Exited {
        name: &'static str,
        reason: &'static str,
    },
    ShutdownTimeout,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exited { name, reason } => write!(f, "Worker {name} terminated: {reason}"),
            Self::ShutdownTimeout => f.write_str("Worker shutdown deadline exceeded"),
        }
    }
}
impl std::error::Error for Error {}

pub(super) struct Workers {
    stopping: watch::Sender<bool>,
    tasks: JoinSet<(&'static str, Result<(), tokio::task::JoinError>)>,
    handles: Vec<(&'static str, AbortHandle, bool)>,
}
impl Workers {
    pub(super) fn new() -> Self {
        let (stopping, _) = watch::channel(false);
        Self {
            stopping,
            tasks: JoinSet::new(),
            handles: Vec::new(),
        }
    }
    pub(super) fn stopping(&self) -> watch::Receiver<bool> {
        self.stopping.subscribe()
    }
    pub(super) fn spawn(
        &mut self,
        name: &'static str,
        graceful: bool,
        task: impl Future<Output = ()> + Send + 'static,
    ) {
        self.adopt(name, graceful, tokio::spawn(task));
    }
    /// Keep the abort handle of extension tasks too: dropping a JoinHandle alone detaches it.
    pub(super) fn adopt(&mut self, name: &'static str, graceful: bool, task: JoinHandle<()>) {
        self.handles.push((name, task.abort_handle(), graceful));
        self.tasks.spawn(async move { (name, task.await) });
    }
    pub(super) async fn failed(&mut self) -> Error {
        if self.handles.is_empty() {
            return std::future::pending().await;
        }
        match self.tasks.join_next().await {
            Some(Ok((name, result))) => Error::Exited {
                name,
                reason: match result {
                    Ok(()) => "unexpected completion",
                    Err(error) if error.is_cancelled() => "cancelled",
                    Err(_) => "panic",
                },
            },
            _ => Error::Exited {
                name: "supervisor",
                reason: "no active monitor",
            },
        }
    }
    /// Signal cooperative workers together; lease-based tasks may be cancelled immediately.
    /// All grace periods share the host's deadline rather than accumulating per task.
    pub(super) async fn shutdown(&mut self, deadline: Instant) -> Result<(), Error> {
        self.stopping.send_replace(true);
        for (_, handle, graceful) in &self.handles {
            if !graceful {
                handle.abort();
            }
        }
        let drain = async {
            let mut failed = None;
            while let Some(result) = self.tasks.join_next().await {
                if let Ok((name, Err(error))) = result {
                    let graceful = self
                        .handles
                        .iter()
                        .any(|(worker, _, graceful)| *worker == name && *graceful);
                    if !error.is_cancelled() || graceful {
                        failed.get_or_insert(Error::Exited {
                            name,
                            reason: if error.is_cancelled() {
                                "unexpected cancellation during shutdown"
                            } else {
                                "panic during shutdown"
                            },
                        });
                    }
                } else if result.is_err() {
                    failed.get_or_insert(Error::Exited {
                        name: "supervisor",
                        reason: "monitor failed",
                    });
                }
            }
            failed.map_or(Ok(()), Err)
        };
        match tokio::time::timeout_at(deadline, drain).await {
            Ok(result) => result,
            Err(_) => {
                for (_, handle, _) in &self.handles {
                    handle.abort();
                }
                // Cancellation is requested for every task. Drop also aborts monitors;
                // blocking work already in flight retains its own permits/lease contract.
                Err(Error::ShutdownTimeout)
            }
        }
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        for (_, handle, _) in &self.handles {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    #[tokio::test]
    async fn observes_completion_and_panic_while_host_is_running() {
        for panic in [false, true] {
            let mut workers = Workers::new();
            workers.spawn("email", true, async move {
                assert!(!panic, "synthetic worker failure");
            });
            assert_eq!(
                workers.failed().await,
                Error::Exited {
                    name: "email",
                    reason: if panic {
                        "panic"
                    } else {
                        "unexpected completion"
                    }
                }
            );
        }
    }
    #[tokio::test]
    async fn graceful_workers_receive_stop_together_and_are_joined() {
        let mut workers = Workers::new();
        for name in ["email", "payments"] {
            let mut stopping = workers.stopping();
            workers.spawn(name, true, async move {
                stopping.changed().await.unwrap();
                assert!(*stopping.borrow());
            });
        }
        workers
            .shutdown(Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
        assert!(workers.tasks.is_empty());
    }
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    #[tokio::test]
    async fn stalled_work_is_cancelled_at_one_shared_deadline() {
        let mut workers = Workers::new();
        let dropped = Arc::new(AtomicBool::new(false));
        let resource = Dropped(dropped.clone());
        workers.spawn("stalled", true, async move {
            let _resource = resource;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        assert_eq!(
            workers
                .shutdown(Instant::now() + Duration::from_millis(20))
                .await,
            Err(Error::ShutdownTimeout)
        );
        tokio::task::yield_now().await;
        assert!(dropped.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn early_host_failure_does_not_detach_extension_tasks() {
        let mut workers = Workers::new();
        let dropped = Arc::new(AtomicBool::new(false));
        let resource = Dropped(dropped.clone());
        workers.adopt(
            "extension",
            false,
            tokio::spawn(async move {
                let _resource = resource;
                std::future::pending::<()>().await;
            }),
        );
        tokio::task::yield_now().await;
        drop(workers);
        tokio::task::yield_now().await;
        assert!(dropped.load(Ordering::SeqCst));
    }
}
