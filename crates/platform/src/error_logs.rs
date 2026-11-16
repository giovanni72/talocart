//! Owner-only, bounded reading of the private JSON error journal.
use crate::identity::{Identity, IdentityError, Session};
use std::{path::PathBuf, sync::Arc};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt},
    sync::Semaphore,
};

const WINDOW: u64 = 256 * 1024;
const PAGE_SIZE: usize = 100;

#[derive(Clone)]
pub struct ErrorLogs {
    root: PathBuf,
    readers: Arc<Semaphore>,
}
pub struct Page {
    pub dates: Vec<String>,
    pub date: String,
    pub entries: Vec<Entry>,
    pub before: Option<u64>,
    pub skipped: bool,
}
pub struct Entry {
    pub timestamp: String,
    pub target: String,
    pub message: String,
    pub details: String,
}
impl ErrorLogs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            readers: Arc::new(Semaphore::new(2)),
        }
    }
    pub async fn read(
        &self,
        identity: &Identity,
        session: &Session,
        date: &str,
        before: Option<u64>,
    ) -> Result<Page, IdentityError> {
        identity.authorize_owner(session).await?;
        let _permit = self
            .readers
            .try_acquire()
            .map_err(|_| IdentityError::Busy)?;
        self.read_files(date, before).await
    }
    async fn read_files(&self, date: &str, before: Option<u64>) -> Result<Page, IdentityError> {
        if !date.is_empty() && !valid_date(date) {
            return Err(IdentityError::Invalid);
        }
        let root = fs::canonicalize(&self.root).await.map_err(storage_error)?;
        let mut directory = fs::read_dir(&root).await.map_err(storage_error)?;
        let mut dates = Vec::new();
        let mut count = 0;
        while let Some(file) = directory.next_entry().await.map_err(storage_error)? {
            count += 1;
            if count > 1024 {
                return Err(IdentityError::Unavailable);
            }
            let name = file.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(day) = name
                .strip_prefix("errors.")
                .and_then(|s| s.strip_suffix(".jsonl"))
                && valid_date(day)
                && file.file_type().await.map_err(storage_error)?.is_file()
            {
                dates.push(day.to_owned());
            }
        }
        dates.sort_unstable_by(|a, b| b.cmp(a));
        let date = if date.is_empty() {
            dates.first().cloned().unwrap_or_default()
        } else {
            date.to_owned()
        };
        let mut page = Page {
            dates,
            date,
            entries: vec![],
            before: None,
            skipped: false,
        };
        if page.date.is_empty() {
            return Ok(page);
        }
        if !page.dates.contains(&page.date) {
            return Err(IdentityError::NotFound);
        }
        let path = root.join(format!("errors.{}.jsonl", page.date));
        let metadata = fs::symlink_metadata(&path).await.map_err(storage_error)?;
        if !metadata.is_file()
            || fs::canonicalize(&path)
                .await
                .map_err(storage_error)?
                .parent()
                != Some(root.as_path())
        {
            return Err(IdentityError::Unavailable);
        }
        let mut file = fs::File::open(path).await.map_err(storage_error)?;
        let length = file.metadata().await.map_err(storage_error)?.len();
        let end = before.unwrap_or(length);
        if end > length {
            return Err(IdentityError::Invalid);
        }
        let start = end.saturating_sub(WINDOW);
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(storage_error)?;
        let mut bytes = Vec::new();
        file.take(end - start)
            .read_to_end(&mut bytes)
            .await
            .map_err(storage_error)?;
        let first = if start > 0 {
            bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |i| i + 1)
        } else {
            0
        };
        // Ignore an unfinished last write. Subsequent refresh sees the complete record.
        let last = bytes
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(first, |i| i + 1);
        let mut offset = start + last as u64;
        for line in bytes[first..last].split_inclusive(|b| *b == b'\n').rev() {
            offset -= line.len() as u64;
            match parse_entry(line) {
                Some(entry) => page.entries.push(entry),
                None => page.skipped = true,
            }
            if page.entries.len() == PAGE_SIZE {
                break;
            }
        }
        if offset > 0 {
            page.before = Some(offset);
        }
        // A single oversized/unterminated record must not trap pagination.
        if page.before == Some(end) || (page.entries.is_empty() && start > 0) {
            page.before = Some(start);
            page.skipped = true;
        }
        Ok(page)
    }
}
fn parse_entry(line: &[u8]) -> Option<Entry> {
    if line.len() > 32 * 1024 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    if value.get("level")?.as_str()? != "ERROR" {
        return None;
    }
    Some(Entry {
        timestamp: value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        target: value
            .get("target")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        message: value
            .pointer("/fields/message")
            .and_then(|v| v.as_str())
            .unwrap_or("ERROR")
            .into(),
        details: serde_json::to_string_pretty(&value).ok()?,
    })
}
fn valid_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
        && ("01"..="12").contains(&&date[5..7])
        && ("01"..="31").contains(&&date[8..10])
}
fn storage_error(error: std::io::Error) -> IdentityError {
    tracing::warn!(kind = ?error.kind(), "Error journal unavailable");
    IdentityError::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "talocart-journal-{}-{}",
            std::process::id(),
            crate::identity::token().expect("random token")
        ));
        std::fs::create_dir_all(&root).expect("fixture directory");
        root
    }
    fn event(n: usize) -> String {
        format!(
            "{}\n",
            serde_json::json!({"timestamp":"2026-10-30T12:00:00Z", "level":"ERROR", "target":"synthetic", "fields":{"message":format!("Event {n} <script>alert(1)</script>")}})
        )
    }
    #[tokio::test]
    async fn pagination_is_stable_when_new_events_are_appended() {
        let root = fixture();
        let path = root.join("errors.2026-10-30.jsonl");
        std::fs::write(&path, (0..205).map(event).collect::<String>()).unwrap();
        let logs = ErrorLogs::new(root.clone());
        let first = logs.read_files("", None).await.unwrap();
        assert_eq!(first.entries.len(), 100);
        assert!(first.entries[0].message.starts_with("Event 204 "));
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(event(205).as_bytes())
            .unwrap();
        let second = logs.read_files(&first.date, first.before).await.unwrap();
        assert_eq!(second.entries.len(), 100);
        assert!(second.entries[0].message.starts_with("Event 104 "));
        let third = logs.read_files(&first.date, second.before).await.unwrap();
        assert_eq!(third.entries.len(), 5);
        assert!(third.entries[0].message.starts_with("Event 4 "));
        assert_eq!(third.before, None);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn invalid_paths_links_partial_and_large_records_are_bounded() {
        let root = fixture();
        let logs = ErrorLogs::new(root.clone());
        assert!(logs.read_files("", None).await.unwrap().entries.is_empty());
        for bad in ["../../etc/passwd", "2026-99-30", "2026-10-30/", "ééééé"] {
            assert!(matches!(
                logs.read_files(bad, None).await,
                Err(IdentityError::Invalid)
            ));
        }
        let path = root.join("errors.2026-10-30.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n{}broken\n{{\"unfinished\"",
                "x".repeat(WINDOW as usize + 1),
                event(1)
            ),
        )
        .unwrap();
        let first = logs.read_files("", None).await.unwrap();
        assert_eq!(first.entries.len(), 1);
        assert!(first.skipped);
        let older = logs.read_files("", first.before).await.unwrap();
        assert!(older.entries.is_empty());
        assert!(older.before < first.before);
        assert!(matches!(
            logs.read_files("", Some(u64::MAX)).await,
            Err(IdentityError::Invalid)
        ));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&path, root.join("errors.2026-10-29.jsonl")).unwrap();
            assert!(matches!(
                logs.read_files("2026-10-29", None).await,
                Err(IdentityError::NotFound)
            ));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
