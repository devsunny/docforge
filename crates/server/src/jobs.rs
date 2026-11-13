//! In-memory job records with on-disk inputs and outputs.

use chrono::{DateTime, Utc};
use docforge_core::format::Format;
use docforge_core::options::ConversionOptions;
use parking_lot::RwLock;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JobRecord {
    pub id: String,
    pub status: JobStatus,
    pub owner: String,
    pub client_id: String,
    pub source_format: Format,
    pub target_format: Format,
    pub input_name: String,
    pub output_name: String,
    pub input_bytes: u64,
    pub output_bytes: Option<u64>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub route: Option<String>,
    pub plan: Option<serde_json::Value>,
    pub error: Option<String>,
    pub download_url: Option<String>,
    #[serde(skip)]
    pub options: ConversionOptions,
}

pub struct JobStore {
    root: PathBuf,
    jobs: RwLock<HashMap<String, JobRecord>>,
}

impl JobStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            jobs: RwLock::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn job_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn input_path(&self, id: &str, extension: &str) -> PathBuf {
        self.job_dir(id).join(format!("input.{extension}"))
    }

    pub fn output_path(&self, id: &str, extension: &str) -> PathBuf {
        self.job_dir(id).join(format!("output.{extension}"))
    }

    pub fn insert(&self, record: JobRecord) {
        self.jobs.write().insert(record.id.clone(), record);
    }

    pub fn get(&self, id: &str) -> Option<JobRecord> {
        self.jobs.read().get(id).cloned()
    }

    pub fn list(
        &self,
        owner: &str,
        status: Option<JobStatus>,
        limit: usize,
        offset: usize,
    ) -> Vec<JobRecord> {
        let mut jobs: Vec<JobRecord> = self
            .jobs
            .read()
            .values()
            .filter(|job| job.owner == owner)
            .filter(|job| status.is_none_or(|status| job.status == status))
            .cloned()
            .collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
        jobs.into_iter().skip(offset).take(limit).collect()
    }

    pub fn update<F>(&self, id: &str, update: F) -> Option<JobRecord>
    where
        F: FnOnce(&mut JobRecord),
    {
        let mut jobs = self.jobs.write();
        let job = jobs.get_mut(id)?;
        update(job);
        Some(job.clone())
    }

    pub fn remove(&self, id: &str) -> Option<JobRecord> {
        self.jobs.write().remove(id)
    }

    pub fn delete_files(&self, id: &str) {
        let _ = std::fs::remove_dir_all(self.job_dir(id));
    }

    pub fn cleanup(&self, ttl: Duration) {
        let now = Utc::now();
        let ttl = chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::hours(24));
        let stale: Vec<String> = self
            .jobs
            .read()
            .values()
            .filter(|job| job.status.is_terminal())
            .filter(|job| now.signed_duration_since(job.created_at) > ttl)
            .map(|job| job.id.clone())
            .collect();
        for id in stale {
            self.remove(id.as_str());
            self.delete_files(id.as_str());
        }
        // Remove orphan directories older than the TTL as well.
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        let modified: DateTime<Utc> = modified.into();
                        if now.signed_duration_since(modified) > ttl {
                            let _ = std::fs::remove_dir_all(path);
                        }
                    }
                }
            }
        }
    }

    pub fn new_record(
        owner: &str,
        client_id: &str,
        source: Format,
        target: Format,
        input_name: String,
        input_bytes: u64,
        download_base: &str,
    ) -> JobRecord {
        let id = Uuid::new_v4().to_string();
        JobRecord {
            id: id.clone(),
            status: JobStatus::Queued,
            owner: owner.to_string(),
            client_id: client_id.to_string(),
            source_format: source,
            target_format: target,
            input_name,
            output_name: String::new(),
            input_bytes,
            output_bytes: None,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            route: None,
            plan: None,
            error: None,
            download_url: Some(format!("{download_base}/api/v1/jobs/{id}/download")),
            options: ConversionOptions::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_scoped_to_owner() {
        let temp = tempfile::tempdir().unwrap();
        let store = JobStore::new(temp.path().join("jobs"));
        let record = JobStore::new_record(
            "alice",
            "client",
            Format::Txt,
            Format::Pdf,
            "a.txt".into(),
            10,
            "http://localhost",
        );
        let id = record.id.clone();
        store.insert(record);
        assert!(store.get(&id).is_some());
        assert_eq!(store.list("alice", None, 10, 0).len(), 1);
        assert_eq!(store.list("bob", None, 10, 0).len(), 0);
    }
}
