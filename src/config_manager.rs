use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// The complete, revisioned configuration state exposed to management clients.
///
/// `Registry` remains the on-disk schema.  Clients must send the revision back
/// to [`ConfigManager::replace`] rather than writing `registry.toml` directly;
/// that preserves cross-process conflict detection and atomic persistence.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigSnapshot {
    pub registry: crate::workspace::Registry,
    pub revision: u64,
}

#[derive(Clone)]
pub struct ConfigManager {
    path: PathBuf,
    cache: Arc<RwLock<ConfigSnapshot>>,
    events: tokio::sync::watch::Sender<ConfigSnapshot>,
}

pub struct ConfigWatcher {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ConfigManager {
    pub fn open() -> Result<Self> {
        Self::at(crate::workspace::Registry::config_path())
    }

    pub fn at(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let snapshot = read_snapshot(&path)?;
        let (events, _) = tokio::sync::watch::channel(snapshot.clone());
        Ok(Self {
            path,
            cache: Arc::new(RwLock::new(snapshot)),
            events,
        })
    }

    pub fn snapshot(&self) -> ConfigSnapshot {
        self.cache
            .read()
            .expect("config cache lock poisoned")
            .clone()
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<ConfigSnapshot> {
        self.events.subscribe()
    }

    /// Watches the parent directory so atomic rename writes are observed as well.
    pub fn watch_file(&self) -> Result<ConfigWatcher> {
        use notify::Watcher;
        let parent = self
            .path
            .parent()
            .context("Configuration path has no parent")?
            .to_path_buf();
        let watched_path = self.path.clone();
        let manager = self.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let (events_tx, events_rx) = std::sync::mpsc::channel();

        let mut watcher = notify::RecommendedWatcher::new(
            move |event| {
                let _ = events_tx.send(event);
            },
            notify::Config::default(),
        )?;
        watcher.watch(&parent, notify::RecursiveMode::NonRecursive)?;
        let handle = std::thread::spawn(move || {
            // Keep the watcher alive for the duration of the worker thread.
            let _watcher = watcher;
            while !thread_stop.load(Ordering::Acquire) {
                let Ok(event) = events_rx.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                if event
                    .ok()
                    .is_some_and(|event| event.paths.iter().any(|path| path == &watched_path))
                    && let Err(error) = manager.reload()
                {
                    tracing::warn!("Ignoring invalid external configuration update: {error}");
                }
            }
        });
        Ok(ConfigWatcher {
            stop,
            handle: Some(handle),
        })
    }

    /// Reloads a valid external change. On parse failure the cached valid snapshot remains active.
    pub fn reload(&self) -> Result<ConfigSnapshot> {
        let snapshot = read_snapshot(&self.path)?;
        let mut cache = self.cache.write().expect("config cache lock poisoned");
        if cache.revision != snapshot.revision || cache.registry != snapshot.registry {
            *cache = snapshot.clone();
            let _ = self.events.send(snapshot.clone());
        }
        Ok(snapshot)
    }

    pub fn replace(
        &self,
        expected_revision: u64,
        registry: crate::workspace::Registry,
    ) -> Result<ConfigSnapshot> {
        let mut lock = ConfigFileLock::acquire(&self.path)?;
        let current = read_snapshot(&self.path)?;
        if current.revision != expected_revision {
            return Err(anyhow!(
                "CONFIG_CONFLICT: expected revision {}, found {}",
                expected_revision,
                current.revision
            ));
        }
        validate(&registry)?;
        let mut registry = registry;
        registry.schema_version = crate::workspace::REGISTRY_SCHEMA_VERSION;
        registry.config_revision = current.revision + 1;
        write_atomic(&self.path, &registry)?;
        lock.release()?;

        let snapshot = ConfigSnapshot {
            revision: registry.config_revision,
            registry,
        };
        *self.cache.write().expect("config cache lock poisoned") = snapshot.clone();
        let _ = self.events.send(snapshot.clone());
        Ok(snapshot)
    }

    /// Deserialize a GUI/MCP payload and atomically replace the configuration.
    ///
    /// This is intentionally kept here (rather than in a UI adapter) so every
    /// management client receives the same validation, file lock, revision
    /// check, and atomic-rename semantics.
    pub fn replace_json(
        &self,
        expected_revision: u64,
        registry: serde_json::Value,
    ) -> Result<ConfigSnapshot> {
        let registry =
            serde_json::from_value(registry).context("Invalid registry configuration payload")?;
        self.replace(expected_revision, registry)
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn load_registry() -> Result<crate::workspace::Registry> {
    Ok(ConfigManager::open()?.snapshot().registry)
}

fn read_snapshot(path: &Path) -> Result<ConfigSnapshot> {
    if !path.exists() {
        return Ok(ConfigSnapshot {
            registry: crate::workspace::Registry::default(),
            revision: 0,
        });
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read configuration {}", path.display()))?;
    let registry = toml::from_str::<crate::workspace::Registry>(&content)
        .with_context(|| format!("Invalid configuration {}", path.display()))?;
    validate(&registry)?;
    Ok(ConfigSnapshot {
        revision: registry.config_revision,
        registry,
    })
}

fn validate(registry: &crate::workspace::Registry) -> Result<()> {
    if registry.schema_version > crate::workspace::REGISTRY_SCHEMA_VERSION {
        return Err(anyhow!(
            "Configuration schema {} is newer than supported schema {}",
            registry.schema_version,
            crate::workspace::REGISTRY_SCHEMA_VERSION
        ));
    }
    for (name, project) in &registry.projects {
        if name.trim().is_empty()
            || project.code_path.trim().is_empty()
            || project.vault_path.trim().is_empty()
        {
            return Err(anyhow!("Invalid project configuration entry"));
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, registry: &crate::workspace::Registry) -> Result<()> {
    let parent = path.parent().context("Configuration path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(toml::to_string_pretty(registry)?.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .ok();
    Ok(())
}

struct ConfigFileLock {
    file: File,
}

impl ConfigFileLock {
    fn acquire(config_path: &Path) -> Result<Self> {
        let parent = config_path
            .parent()
            .context("Configuration path has no parent")?;
        std::fs::create_dir_all(parent)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(parent.join(".registry.lock"))?;
        file.lock_exclusive()?;
        Ok(Self { file })
    }

    fn release(&mut self) -> Result<()> {
        self.file.unlock()?;
        Ok(())
    }
}

impl Drop for ConfigFileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_is_revisioned_and_notifies_subscribers() {
        let directory = tempfile::tempdir().unwrap();
        let manager = ConfigManager::at(directory.path().join("registry.toml")).unwrap();
        let mut subscriber = manager.subscribe();
        let mut registry = manager.snapshot().registry;
        registry.global.max_backups = Some(7);
        let updated = manager.replace(0, registry).unwrap();
        assert_eq!(updated.revision, 1);
        assert_eq!(subscriber.borrow_and_update().revision, 1);
        assert!(manager.replace(0, updated.registry).is_err());
    }

    #[test]
    fn malformed_external_file_does_not_replace_cached_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("registry.toml");
        let manager = ConfigManager::at(&path).unwrap();
        let registry = manager.snapshot().registry;
        manager.replace(0, registry).unwrap();
        std::fs::write(&path, "not valid = [").unwrap();
        assert!(manager.reload().is_err());
        assert_eq!(manager.snapshot().revision, 1);
    }

    #[test]
    fn snapshot_is_serializable_and_json_replace_is_revisioned() {
        let directory = tempfile::tempdir().unwrap();
        let manager = ConfigManager::at(directory.path().join("registry.toml")).unwrap();
        let snapshot = manager.snapshot();

        let mut registry = serde_json::to_value(&snapshot.registry).unwrap();
        registry["global"]["max_backups"] = serde_json::json!(9);
        let updated = manager.replace_json(snapshot.revision, registry).unwrap();

        let client_payload = serde_json::to_value(&updated).unwrap();
        assert_eq!(client_payload["revision"], 1);
        assert_eq!(client_payload["registry"]["global"]["max_backups"], 9);
        assert!(
            manager
                .replace_json(0, client_payload["registry"].clone())
                .unwrap_err()
                .to_string()
                .starts_with("CONFIG_CONFLICT:")
        );
    }
}
