use anyhow::Result;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockOwner {
    pub pid: u32,
    pub acquired_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockInspection {
    Active(Option<LockOwner>),
    Unlocked,
    StaleMetadataCleared(LockOwner),
}

pub struct IndexLock {
    file: File,
}

fn open_lock_file(storage_path: &str) -> Result<File> {
    std::fs::create_dir_all(storage_path)?;
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(Path::new(storage_path).join(".index.lock"))?)
}

fn read_owner(file: &mut File) -> Option<LockOwner> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut metadata = String::new();
    file.read_to_string(&mut metadata).ok()?;
    serde_json::from_str(metadata.trim()).ok()
}

fn write_owner(file: &mut File) -> Result<()> {
    let owner = LockOwner {
        pid: std::process::id(),
        acquired_at: chrono::Utc::now().to_rfc3339(),
    };
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    serde_json::to_writer(&mut *file, &owner)?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    Ok(())
}

pub fn try_acquire(storage_path: &str) -> Result<Option<IndexLock>> {
    let mut file = open_lock_file(storage_path)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            write_owner(&mut file)?;
            Ok(Some(IndexLock { file }))
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub async fn acquire(storage_path: &str) -> Result<IndexLock> {
    loop {
        if let Some(lock) = try_acquire(storage_path)? {
            return Ok(lock);
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }
}

pub fn inspect(storage_path: &str) -> Result<LockInspection> {
    let mut file = open_lock_file(storage_path)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            let stale_owner = read_owner(&mut file);
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            file.sync_data()?;
            file.unlock()?;
            Ok(match stale_owner {
                Some(owner) => LockInspection::StaleMetadataCleared(owner),
                None => LockInspection::Unlocked,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            Ok(LockInspection::Active(read_owner(&mut file)))
        }
        Err(error) => Err(error.into()),
    }
}

impl Drop for IndexLock {
    fn drop(&mut self) {
        let _ = self.file.set_len(0);
        let _ = self.file.seek(SeekFrom::Start(0));
        let _ = self.file.sync_data();
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    #[test]
    fn lock_records_owner_and_inspection_respects_os_lock() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path().to_string_lossy();

        let lock = try_acquire(&storage).unwrap().unwrap();
        match inspect(&storage).unwrap() {
            LockInspection::Active(Some(owner)) => assert_eq!(owner.pid, std::process::id()),
            other => panic!("unexpected inspection: {other:?}"),
        }

        drop(lock);
        assert_eq!(inspect(&storage).unwrap(), LockInspection::Unlocked);
    }

    #[test]
    fn inspection_clears_stale_metadata_only_after_acquiring_lock() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path().to_string_lossy();
        let owner = LockOwner {
            pid: 424_242,
            acquired_at: "2026-07-13T00:00:00Z".to_string(),
        };
        std::fs::write(
            dir.path().join(".index.lock"),
            format!("{}\n", serde_json::to_string(&owner).unwrap()),
        )
        .unwrap();

        assert_eq!(
            inspect(&storage).unwrap(),
            LockInspection::StaleMetadataCleared(owner)
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".index.lock")).unwrap(),
            ""
        );
    }

    #[test]
    fn child_process_holds_lock_for_crash_test() {
        if std::env::var_os("RMS_INDEX_LOCK_TEST_CHILD").is_none() {
            return;
        }

        let storage = std::env::var("RMS_INDEX_LOCK_TEST_STORAGE").unwrap();
        let _lock = try_acquire(&storage).unwrap().unwrap();
        println!("RMS_INDEX_LOCK_ACQUIRED");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::park();
        }
    }

    #[test]
    fn child_process_contends_for_multi_writer_test() {
        if std::env::var_os("RMS_INDEX_LOCK_MULTI_WRITER_CHILD").is_none() {
            return;
        }
        let storage = std::env::var("RMS_INDEX_LOCK_TEST_STORAGE").unwrap();
        let events = std::env::var("RMS_INDEX_LOCK_TEST_EVENTS").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let lock = runtime.block_on(acquire(&storage)).unwrap();
        append_event(&events, format!("start:{}", std::process::id()));
        std::thread::sleep(std::time::Duration::from_millis(100));
        append_event(&events, format!("end:{}", std::process::id()));
        drop(lock);
    }

    fn append_event(path: &str, event: String) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{event}").unwrap();
        file.sync_data().unwrap();
    }

    #[test]
    fn os_releases_lock_after_owner_process_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path().to_string_lossy().into_owned();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("index_lock::tests::child_process_holds_lock_for_crash_test")
            .arg("--exact")
            .arg("--nocapture")
            .env("RMS_INDEX_LOCK_TEST_CHILD", "1")
            .env("RMS_INDEX_LOCK_TEST_STORAGE", &storage)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();

        let stdout = child.stdout.take().unwrap();
        let mut acquired = false;
        for line in BufReader::new(stdout).lines() {
            if line.unwrap().contains("RMS_INDEX_LOCK_ACQUIRED") {
                acquired = true;
                break;
            }
        }
        assert!(acquired, "child process did not acquire the index lock");

        let owner = match inspect(&storage).unwrap() {
            LockInspection::Active(Some(owner)) => owner,
            other => panic!("expected active child lock, got {other:?}"),
        };
        assert_eq!(owner.pid, child.id());

        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            inspect(&storage).unwrap(),
            LockInspection::StaleMetadataCleared(owner)
        );
        assert_eq!(inspect(&storage).unwrap(), LockInspection::Unlocked);
    }

    #[test]
    fn three_processes_serialize_writer_sections() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path().to_string_lossy().into_owned();
        let events = dir.path().join("events.log");
        let children = (0..3)
            .map(|_| {
                Command::new(std::env::current_exe().unwrap())
                    .arg("index_lock::tests::child_process_contends_for_multi_writer_test")
                    .arg("--exact")
                    .env("RMS_INDEX_LOCK_MULTI_WRITER_CHILD", "1")
                    .env("RMS_INDEX_LOCK_TEST_STORAGE", &storage)
                    .env("RMS_INDEX_LOCK_TEST_EVENTS", &events)
                    .spawn()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        let events = std::fs::read_to_string(events)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 6);
        for pair in events.chunks_exact(2) {
            let started = pair[0].strip_prefix("start:").unwrap();
            let ended = pair[1].strip_prefix("end:").unwrap();
            assert_eq!(started, ended, "writer sections overlapped: {pair:?}");
        }
        assert_eq!(inspect(&storage).unwrap(), LockInspection::Unlocked);
    }

    #[tokio::test]
    async fn readers_remain_available_while_a_writer_lock_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path().to_string_lossy().into_owned();
        let _writer = try_acquire(&storage).unwrap().unwrap();
        let store = crate::store::Store::init(&storage, "memory").await.unwrap();
        let (table, _) = store.open_or_create_code_table().await.unwrap();
        assert_eq!(table.count_rows(None).await.unwrap(), 0);
        assert!(
            store
                .search_code(vec![0.0; crate::store::VECTOR_DIMENSION], 1)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
