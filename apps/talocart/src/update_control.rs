//! Private, root-peer-only control of the live package manager. No shell or paths in RPC.
use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf, time::Duration};
use talocart_platform::packages::{Manager, UpdateSnapshot};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Inventory {
        configuration_sha256: String,
    },
    Quiesce {
        inventory_sha256: String,
        configuration_sha256: String,
    },
    Resume {
        configuration_sha256: String,
    },
    Warm {
        inventory_sha256: String,
        configuration_sha256: String,
    },
}
impl Request {
    fn configuration_sha256(&self) -> &str {
        match self {
            Self::Inventory {
                configuration_sha256,
            }
            | Self::Quiesce {
                configuration_sha256,
                ..
            }
            | Self::Resume {
                configuration_sha256,
            }
            | Self::Warm {
                configuration_sha256,
                ..
            } => configuration_sha256,
        }
    }
}

#[derive(Serialize)]
struct Reply {
    ok: bool,
    snapshot: Option<UpdateSnapshot>,
    error: Option<&'static str>,
    configuration_sha256: Option<String>,
}
pub(crate) struct Control {
    listener: UnixListener,
    manager: Manager,
    configuration_sha256: Option<String>,
}
impl Control {
    pub(crate) async fn bind(
        root: PathBuf,
        manager: Manager,
        configuration_sha256: Option<String>,
    ) -> io::Result<Self> {
        let socket = root.join("requests/control.sock");
        let listener = tokio::task::spawn_blocking(move || {
            use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
            let parent = socket
                .parent()
                .ok_or_else(|| io::Error::other("Invalid update control directory"))?;
            let meta = std::fs::symlink_metadata(parent)?;
            if !meta.is_dir() || meta.file_type().is_symlink() || meta.mode() & 0o077 != 0 {
                return Err(io::Error::other("Update control directory must be private"));
            }
            match std::fs::symlink_metadata(&socket) {
                Ok(info) if info.file_type().is_socket() && info.uid() == meta.uid() => {
                    std::fs::remove_file(&socket)?
                }
                Ok(_) => return Err(io::Error::other("Unexpected update control path")),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            let listener = std::os::unix::net::UnixListener::bind(&socket)?;
            std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            Ok(listener)
        })
        .await
        .map_err(|_| io::Error::other("Update control initialization failed"))??;
        Ok(Self {
            listener: UnixListener::from_std(listener)?,
            manager,
            configuration_sha256,
        })
    }
    pub(crate) async fn run(self) {
        loop {
            let stream = match self.listener.accept().await {
                Ok((stream, _)) => stream,
                Err(_) => {
                    tracing::error!("Update control accept failed");
                    return;
                }
            };
            // One bounded request at a time; peer credentials come from the kernel.
            // No unauthenticated request reaches the manager.
            let _ = tokio::time::timeout(
                Duration::from_secs(600),
                serve(stream, &self.manager, self.configuration_sha256.clone()),
            )
            .await;
        }
    }
}
async fn execute(
    request: Request,
    manager: &Manager,
    configuration_sha256: Option<String>,
) -> Reply {
    if configuration_sha256.as_deref() != Some(request.configuration_sha256()) {
        return Reply {
            ok: false,
            snapshot: manager.update_snapshot().ok(),
            error: Some("configuration"),
            configuration_sha256,
        };
    }
    let result = match request {
        Request::Inventory { .. } => manager.update_snapshot(),
        Request::Quiesce {
            inventory_sha256, ..
        } => manager.prepare_update(inventory_sha256).await,
        Request::Resume { .. } => manager.resume_update().await,
        Request::Warm {
            inventory_sha256, ..
        } => manager.warm_update(inventory_sha256).await,
    };
    match result {
        Ok(snapshot) => Reply {
            ok: true,
            snapshot: Some(snapshot),
            error: None,
            configuration_sha256,
        },
        Err(error) => {
            use talocart_platform::packages::Error;
            let code = match error {
                Error::Busy => "busy",
                Error::Conflict => "conflict",
                _ => "unavailable",
            };
            Reply {
                ok: false,
                snapshot: manager.update_snapshot().ok(),
                error: Some(code),
                configuration_sha256,
            }
        }
    }
}
async fn serve(
    stream: UnixStream,
    manager: &Manager,
    configuration_sha256: Option<String>,
) -> io::Result<()> {
    if stream.peer_cred()?.uid() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Root updater peer required",
        ));
    }
    let (reader, mut writer) = stream.into_split();
    let mut raw = Vec::new();
    let read = tokio::time::timeout(
        Duration::from_secs(5),
        BufReader::new(reader)
            .take(4097)
            .read_until(b'\n', &mut raw),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Update control input timed out"))??;
    if read == 0 || read > 4096 || raw.last() != Some(&b'\n') {
        return Err(io::Error::other("Invalid update control message"));
    }
    let request = serde_json::from_slice::<Request>(&raw)
        .map_err(|_| io::Error::other("Invalid update control message"))?;
    let mut reply = serde_json::to_vec(&execute(request, manager, configuration_sha256).await)
        .map_err(|_| io::Error::other("Update control response failed"))?;
    reply.push(b'\n');
    writer.write_all(&reply).await
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn manager() -> (Manager, PathBuf) {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "talocart-control-{}-{}",
                std::process::id(),
                talocart_platform::identity::token().unwrap()
            ));
        let manager = Manager::open(talocart_platform::packages::RuntimeConfig {
            root: root.join("packages"),
            media_root: root.join("media"),
            database_url: "postgres://unused@127.0.0.1:1/control_test".into(),
            store_name: "fixture".into(),
            public_origin: "http://127.0.0.1:3000".into(),
            secure_cookies: false,
            compiled: vec![],
            serve: true,
        })
        .await
        .unwrap();
        (manager, root)
    }
    #[tokio::test]
    async fn socket_uses_kernel_peer_credentials_and_rejects_non_root() {
        let (manager, root) = manager().await;
        let (server, mut client) = UnixStream::pair().unwrap();
        let uid = server.peer_cred().unwrap().uid();
        let message = serde_json::to_vec(
            &serde_json::json!({"operation":"inventory", "configuration_sha256":"a".repeat(64)}),
        )
        .unwrap();
        client
            .write_all(&[message, b"\n".to_vec()].concat())
            .await
            .unwrap();
        let result = serve(server, &manager, Some("a".repeat(64))).await;
        if uid != 0 {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        } else {
            result.unwrap();
            let mut raw = Vec::new();
            client.read_to_end(&mut raw).await.unwrap();
            let reply: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            assert_eq!(reply["ok"], true);
        }
        assert!(!manager.update_snapshot().unwrap().suspended);
        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn changed_startup_configuration_is_rejected_before_quiescing_the_host() {
        let (manager, root) = manager().await;
        let inventory_sha256 = manager.update_snapshot().unwrap().inventory_sha256;
        let reply = execute(
            Request::Quiesce {
                inventory_sha256,
                configuration_sha256: "b".repeat(64),
            },
            &manager,
            Some("a".repeat(64)),
        )
        .await;
        assert!(!reply.ok);
        assert_eq!(reply.error, Some("configuration"));
        assert!(!manager.update_snapshot().unwrap().suspended);
        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn control_messages_are_typed_and_reject_paths_commands_and_unknown_fields() {
        for message in [
            r#"{"operation":"inventory","configuration_sha256":"synthetic"}"#,
            r#"{"operation":"resume","configuration_sha256":"synthetic"}"#,
            r#"{"operation":"warm","inventory_sha256":"synthetic","configuration_sha256":"synthetic"}"#,
            r#"{"operation":"quiesce","inventory_sha256":"synthetic","configuration_sha256":"synthetic"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(message).is_ok());
        }
        for message in [
            r#"{"operation":"shell","command":"id"}"#,
            r#"{"operation":"inventory","path":"/etc/shadow"}"#,
            r#"{"operation":"quiesce"}"#,
            r#"{"operation":"warm","inventory_sha256":"synthetic","path":"/etc/shadow"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(message).is_err());
        }
    }
}
