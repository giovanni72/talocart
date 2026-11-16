//! Package tools: these run only in the module author's build environment.
use super::{Error, Manifest, Result, format};
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;
fn output(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| Error::Storage)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| Error::Storage)
}
pub fn keygen(path: &Path) -> Result<String> {
    let document = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
        .map_err(|_| Error::Execution)?;
    let pair = Ed25519KeyPair::from_pkcs8(document.as_ref()).map_err(|_| Error::Execution)?;
    output(path, document.as_ref())?;
    Ok(format::hex(pair.public_key().as_ref()))
}
/// Runs `--describe-package` without credentials or environment; bounded output and time.
pub(super) async fn describe(executable: &Path) -> Result<Manifest> {
    let mut child = tokio::process::Command::new(executable)
        .arg("--describe-package")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Error::Execution)?;
    let stdout = child.stdout.take().ok_or(Error::Execution)?;
    let describe = async {
        let mut raw = Vec::new();
        stdout
            .take(32769)
            .read_to_end(&mut raw)
            .await
            .map_err(|_| Error::Execution)?;
        if raw.len() > 32768 || !child.wait().await.map_err(|_| Error::Execution)?.success() {
            return Err(Error::Execution);
        }
        serde_json::from_slice::<Manifest>(&raw).map_err(|_| Error::Invalid)
    };
    tokio::time::timeout(Duration::from_secs(120), describe)
        .await
        .map_err(|_| Error::Execution)?
}
pub async fn package(executable: &Path, key: &Path, destination: &Path) -> Result<()> {
    let manifest = describe(executable).await?;
    let executable = executable.to_owned();
    let key = key.to_owned();
    let destination = destination.to_owned();
    tokio::task::spawn_blocking(move || {
        if fs::metadata(&executable).map_err(|_| Error::Storage)?.len() > format::LIMIT as u64
            || fs::metadata(&key).map_err(|_| Error::Storage)?.len() > 1024
        {
            return Err(Error::Invalid);
        }
        let bytes = fs::read(executable).map_err(|_| Error::Storage)?;
        let secret = fs::read(key).map_err(|_| Error::Storage)?;
        output(&destination, &format::sign(manifest, &bytes, &secret)?)
    })
    .await
    .map_err(|_| Error::Execution)?
}
/// Create the current package envelope without signing keys.
pub async fn bundle(executable: &Path, destination: &Path) -> Result<()> {
    let manifest = describe(executable).await?;
    let executable = executable.to_owned();
    let destination = destination.to_owned();
    tokio::task::spawn_blocking(move || {
        if fs::metadata(&executable).map_err(|_| Error::Storage)?.len() > format::LIMIT as u64 {
            return Err(Error::Invalid);
        }
        output(
            &destination,
            &format::pack(manifest, &fs::read(executable).map_err(|_| Error::Storage)?)?,
        )
    })
    .await
    .map_err(|_| Error::Execution)?
}
