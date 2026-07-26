use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(String);

impl JobId {
    fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    ReindexVault,
    ReindexCode,
    ReindexAll,
    GraphReconcile,
    Custom(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobProgress {
    pub phase: String,
    pub completed: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub id: JobId,
    pub kind: JobKind,
    pub state: JobState,
    pub progress: Option<JobProgress>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Optional structured metadata about changed entities (files, paths, etc.).
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoreEvent {
    JobCreated {
        job: JobSnapshot,
    },
    JobProgressed {
        job: JobSnapshot,
    },
    JobFinished {
        job: JobSnapshot,
    },
    ConfigChanged {
        revision: u64,
    },
    GraphChanged {
        node_keys: Vec<String>,
        edge_keys: Vec<String>,
    },
}

pub trait JobService: Send + Sync {
    fn start_job(&self, kind: JobKind) -> JobHandle;
    fn snapshot(&self, id: &JobId) -> Option<JobSnapshot>;
    fn snapshots(&self) -> Vec<JobSnapshot>;
    fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<CoreEvent>;
}

#[derive(Clone)]
pub struct JobManager {
    jobs: Arc<Mutex<HashMap<JobId, JobEntry>>>,
    events: tokio::sync::broadcast::Sender<CoreEvent>,
}

struct JobEntry {
    snapshot: JobSnapshot,
}

pub struct JobHandle {
    manager: JobManager,
    id: JobId,
    cancelled: Arc<AtomicBool>,
}

impl JobManager {
    pub fn new() -> Self {
        let (events, _) = tokio::sync::broadcast::channel(256);
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            events,
        }
    }

    pub fn publish(&self, event: CoreEvent) {
        let _ = self.events.send(event);
    }

    fn update<F>(&self, id: &JobId, event: fn(JobSnapshot) -> CoreEvent, update: F) -> Result<()>
    where
        F: FnOnce(&mut JobSnapshot) -> Result<()>,
    {
        let snapshot = {
            let mut jobs = self.jobs.lock().expect("job manager lock poisoned");
            let entry = jobs
                .get_mut(id)
                .ok_or_else(|| anyhow!("Unknown job {}", id.as_str()))?;
            update(&mut entry.snapshot)?;
            entry.snapshot.updated_at = now();
            entry.snapshot.clone()
        };
        self.publish(event(snapshot));
        Ok(())
    }
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

impl JobService for JobManager {
    fn start_job(&self, kind: JobKind) -> JobHandle {
        let id = JobId::new();
        let snapshot = JobSnapshot {
            id: id.clone(),
            kind,
            state: JobState::Queued,
            progress: None,
            error: None,
            created_at: now(),
            updated_at: now(),
            metadata: None,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        self.jobs.lock().expect("job manager lock poisoned").insert(
            id.clone(),
            JobEntry {
                snapshot: snapshot.clone(),
            },
        );
        self.publish(CoreEvent::JobCreated { job: snapshot });
        JobHandle {
            manager: self.clone(),
            id,
            cancelled,
        }
    }

    fn snapshot(&self, id: &JobId) -> Option<JobSnapshot> {
        self.jobs
            .lock()
            .expect("job manager lock poisoned")
            .get(id)
            .map(|entry| entry.snapshot.clone())
    }

    fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<CoreEvent> {
        self.events.subscribe()
    }

    /// Return all current job snapshots (most recent first).
    fn snapshots(&self) -> Vec<JobSnapshot> {
        let mut snapshots: Vec<_> = self
            .jobs
            .lock()
            .expect("job manager lock poisoned")
            .values()
            .map(|entry| entry.snapshot.clone())
            .collect();
        snapshots.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        snapshots
    }
}

impl JobHandle {
    pub fn id(&self) -> &JobId {
        &self.id
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn mark_running(&self) -> Result<()> {
        self.manager.update(
            &self.id,
            |job| CoreEvent::JobProgressed { job },
            |job| {
                ensure_active(job)?;
                job.state = JobState::Running;
                Ok(())
            },
        )
    }

    pub fn report(&self, progress: JobProgress) -> Result<()> {
        if self.is_cancelled() {
            return Err(anyhow!("JOB_CANCELLED: {}", self.id.as_str()));
        }
        self.manager.update(
            &self.id,
            |job| CoreEvent::JobProgressed { job },
            |job| {
                ensure_active(job)?;
                job.state = JobState::Running;
                job.progress = Some(progress);
                Ok(())
            },
        )
    }

    /// Attach structured metadata to this job (e.g. list of changed files).
    pub fn set_metadata(&self, metadata: serde_json::Value) -> Result<()> {
        self.manager.update(
            &self.id,
            |job| CoreEvent::JobProgressed { job },
            |job| {
                ensure_active(job)?;
                job.metadata = Some(metadata);
                Ok(())
            },
        )
    }

    pub fn succeed(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(anyhow!("JOB_CANCELLED: {}", self.id.as_str()));
        }
        self.manager.update(
            &self.id,
            |job| CoreEvent::JobFinished { job },
            |job| {
                ensure_active(job)?;
                job.state = JobState::Succeeded;
                job.error = None;
                Ok(())
            },
        )
    }

    pub fn fail(&self, error: impl Into<String>) -> Result<()> {
        let error = error.into();
        self.manager.update(
            &self.id,
            |job| CoreEvent::JobFinished { job },
            |job| {
                ensure_active(job)?;
                job.state = JobState::Failed;
                job.error = Some(error);
                Ok(())
            },
        )
    }

    pub fn cancel(&self) -> Result<()> {
        self.cancelled.store(true, Ordering::Release);
        self.manager.update(
            &self.id,
            |job| CoreEvent::JobFinished { job },
            |job| {
                ensure_active(job)?;
                job.state = JobState::Cancelled;
                Ok(())
            },
        )
    }
}

fn ensure_active(job: &JobSnapshot) -> Result<()> {
    if matches!(job.state, JobState::Queued | JobState::Running) {
        Ok(())
    } else {
        Err(anyhow!(
            "JOB_NOT_ACTIVE: {} is {:?}",
            job.id.as_str(),
            job.state
        ))
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn jobs_emit_progress_and_support_cancellation() {
        let manager = JobManager::new();
        let mut events = manager.subscribe_events();
        let job = manager.start_job(JobKind::ReindexCode);
        assert!(matches!(
            events.recv().await.unwrap(),
            CoreEvent::JobCreated { .. }
        ));
        job.mark_running().unwrap();
        job.report(JobProgress {
            phase: "embedding".to_string(),
            completed: 8,
            total: Some(12),
            message: Some("batch 1".to_string()),
        })
        .unwrap();
        assert!(matches!(
            manager.snapshot(job.id()).unwrap().state,
            JobState::Running
        ));
        job.cancel().unwrap();
        assert!(job.is_cancelled());
        assert!(
            job.report(JobProgress {
                phase: "embedding".to_string(),
                completed: 9,
                total: Some(12),
                message: None,
            })
            .is_err()
        );
        assert_eq!(
            manager.snapshot(job.id()).unwrap().state,
            JobState::Cancelled
        );
        assert!(
            job.succeed()
                .unwrap_err()
                .to_string()
                .contains("JOB_CANCELLED")
        );
    }
}
