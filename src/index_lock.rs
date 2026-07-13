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
}
