//! Persists schedule definitions, scheduling checkpoints, and queued runs for each account.
//! Provides operations to update schedules, enqueue due occurrences, and track run statuses.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::{Occurrence, Schedule, ScheduleError};
use crate::account::AccountId;
use crate::settings::account_config_dir;

const VERSION: u32 = 1;

// Allow normal timer jitter before classifying an automatic run as late.
const LATE_GRACE: Duration = Duration::seconds(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Accepted,
    Running,
    Succeeded,
    Failed,
    Interrupted,
    Skipped,
}

impl RunStatus {
    fn allows(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (Self::Pending, Self::Accepted | Self::Skipped)
                    | (
                        Self::Accepted,
                        Self::Running | Self::Succeeded | Self::Failed | Self::Interrupted
                    )
                    | (
                        Self::Running,
                        Self::Succeeded | Self::Failed | Self::Interrupted
                    )
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedRun {
    pub run_id: String,
    pub occurrence: Occurrence,
    /// Whether this run was first queued by an explicit manual request.
    #[serde(default)]
    pub manual: bool,
    /// Queued more than the grace period after its scheduled time; fixed at creation.
    #[serde(default)]
    pub late: bool,
    pub status: RunStatus,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ScheduleEntry {
    schedule: Schedule,
    checked_through: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    schedules: BTreeMap<String, ScheduleEntry>,
    runs: BTreeMap<String, QueuedRun>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: VERSION,
            schedules: BTreeMap::new(),
            runs: BTreeMap::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("schedule storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid schedule storage JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
    #[error("unsupported schedule storage version: {0}")]
    Version(u64),
    #[error("invalid schedule storage: {0}")]
    InvalidState(String),
    #[error("duplicate schedule id: {0}")]
    DuplicateSchedule(String),
    #[error("timing revision must increase when cadence changes and must never decrease: {0}")]
    RevisionConflict(String),
    #[error("unknown run: {0}")]
    UnknownRun(String),
    #[error("invalid run transition from {from:?} to {to:?}")]
    InvalidTransition { from: RunStatus, to: RunStatus },
}

pub struct ScheduleStore {
    path: PathBuf,
    state: Snapshot,
}

impl ScheduleStore {
    pub fn open_for_account(app: &tauri::AppHandle, account: &AccountId) -> Result<Self, String> {
        let path = account_config_dir(app, account)?.join("schedules.json");
        Self::open(path).map_err(|e| e.to_string())
    }

    pub(super) fn open(path: PathBuf) -> Result<Self, StoreError> {
        let state = match fs::read(&path) {
            Ok(bytes) => {
                // Check version before decoding the rest: a newer schema may differ entirely.
                let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                let version = value
                    .get("version")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| StoreError::InvalidState("missing version".into()))?;
                if version != u64::from(VERSION) {
                    return Err(StoreError::Version(version));
                }
                let snapshot: Snapshot = serde_json::from_value(value)?;
                validate(&snapshot)?;
                snapshot
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Snapshot::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, state })
    }

    /// A complete, successfully fetched snapshot for this account and device. Do not call
    /// this with an empty list to represent a failed fetch. New/re-enabled/revised schedules
    /// activate at `now`; unchanged schedules retain their progress for missed-run recovery.
    pub fn replace_schedules(
        &mut self,
        schedules: Vec<Schedule>,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let mut candidate = self.state.clone();
        candidate.schedules.clear();
        for schedule in schedules {
            schedule.rule()?;
            if candidate.schedules.contains_key(&schedule.id) {
                return Err(StoreError::DuplicateSchedule(schedule.id));
            }
            let previous = self.state.schedules.get(&schedule.id);
            if let Some(previous) = previous {
                let old = &previous.schedule;
                if schedule.timing_revision < old.timing_revision
                    || (schedule.timing_revision == old.timing_revision
                        && schedule.timing != old.timing
                        && !schedule.timing.same_local_cadence(&old.timing))
                {
                    return Err(StoreError::RevisionConflict(schedule.id));
                }
            }
            let checked_through = match previous {
                Some(previous)
                    if previous.schedule.timing_revision == schedule.timing_revision
                        && previous.schedule.timing == schedule.timing
                        && previous.schedule.enabled
                        && schedule.enabled =>
                {
                    previous.checked_through
                }
                Some(previous) => previous.checked_through.max(now),
                None => now,
            };
            candidate.schedules.insert(
                schedule.id.clone(),
                ScheduleEntry {
                    schedule,
                    checked_through,
                },
            );
        }
        // Work already accepted by web needs reconciliation, never automatic cancellation/replay.
        for run in candidate
            .runs
            .values_mut()
            .filter(|r| r.status == RunStatus::Pending)
        {
            let valid = candidate
                .schedules
                .get(&run.occurrence.schedule_id)
                .is_some_and(|entry| {
                    entry.schedule.enabled
                        && entry.schedule.timing_revision == run.occurrence.timing_revision
                        && self
                            .state
                            .schedules
                            .get(&entry.schedule.id)
                            .is_none_or(|previous| {
                                previous.schedule.timing == entry.schedule.timing
                            })
                });
            if !valid {
                run.status = RunStatus::Skipped;
                run.updated_at = run.updated_at.max(now);
            }
        }
        self.commit(candidate)
    }

    /// Manual runs use occurrence identity without advancing the recurring checkpoint.
    pub fn enqueue_manual(
        &mut self,
        schedule_id: &str,
        now: DateTime<Utc>,
    ) -> Result<QueuedRun, StoreError> {
        let entry = self
            .state
            .schedules
            .get(schedule_id)
            .filter(|entry| entry.schedule.enabled)
            .ok_or_else(|| StoreError::InvalidState("schedule is missing or disabled".into()))?;
        let occurrence = Occurrence {
            schedule_id: schedule_id.into(),
            timing_revision: entry.schedule.timing_revision,
            scheduled_for: now,
        };
        let id = run_id(&occurrence);
        // An identical occurrence is the same run, regardless of how it was requested.
        // Preserve its delivery/execution status instead of resetting it to pending.
        if let Some(existing) = self.state.runs.get(&id) {
            return Ok(existing.clone());
        }
        let run = QueuedRun {
            run_id: id,
            occurrence,
            manual: true,
            late: false,
            status: RunStatus::Pending,
            updated_at: now,
        };
        let mut candidate = self.state.clone();
        candidate.runs.insert(run.run_id.clone(), run.clone());
        self.commit(candidate)?;
        Ok(run)
    }

    /// Checkpoints and new pending runs are committed together, before the caller emits events.
    pub fn enqueue_due(&mut self, now: DateTime<Utc>) -> Result<Vec<QueuedRun>, StoreError> {
        let mut candidate = self.state.clone();
        let mut added = vec![];
        for entry in candidate.schedules.values_mut() {
            let plan = entry.schedule.plan(entry.checked_through, now)?;
            for occurrence in plan.occurrences {
                let id = run_id(&occurrence);
                if let std::collections::btree_map::Entry::Vacant(slot) = candidate.runs.entry(id) {
                    let run = QueuedRun {
                        run_id: slot.key().clone(),
                        late: now.signed_duration_since(occurrence.scheduled_for) > LATE_GRACE,
                        occurrence,
                        manual: false,
                        status: RunStatus::Pending,
                        updated_at: now,
                    };
                    slot.insert(run.clone());
                    added.push(run);
                }
            }
            entry.checked_through = plan.checked_through;
        }
        self.commit(candidate)?;
        sort_runs(&mut added);
        Ok(added)
    }

    /// Earliest unprocessed occurrence. Search from each durable checkpoint, not wall-clock
    /// time: an occurrence that became due during I/O must still trigger an immediate check.
    pub(super) fn next_due(&self) -> Result<Option<DateTime<Utc>>, StoreError> {
        let mut next = None;
        for entry in self.state.schedules.values() {
            if let Some(at) = entry.schedule.next_after(entry.checked_through)? {
                next = Some(next.map_or(at, |previous: DateTime<Utc>| previous.min(at)));
            }
        }
        Ok(next)
    }

    /// Reading pending work never removes it. Redelivery retains the same run ID.
    pub fn pending_runs(&self) -> Vec<QueuedRun> {
        let mut runs: Vec<_> = self
            .state
            .runs
            .values()
            .filter(|r| r.status == RunStatus::Pending)
            .cloned()
            .collect();
        sort_runs(&mut runs);
        runs
    }

    pub(super) fn is_unsettled(&self, run_id: &str) -> bool {
        self.state
            .runs
            .get(run_id)
            .is_some_and(|run| matches!(run.status, RunStatus::Accepted | RunStatus::Running))
    }

    /// Accepted/running work survives restart for explicit reconciliation with web.
    pub fn unsettled_runs(&self) -> Vec<QueuedRun> {
        let mut runs: Vec<_> = self
            .state
            .runs
            .values()
            .filter(|r| matches!(r.status, RunStatus::Accepted | RunStatus::Running))
            .cloned()
            .collect();
        sort_runs(&mut runs);
        runs
    }

    pub fn report_status(
        &mut self,
        id: &str,
        status: RunStatus,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let mut candidate = self.state.clone();
        let run = candidate
            .runs
            .get_mut(id)
            .ok_or_else(|| StoreError::UnknownRun(id.into()))?;
        if !run.status.allows(status) {
            return Err(StoreError::InvalidTransition {
                from: run.status,
                to: status,
            });
        }
        if run.status == status {
            return Ok(());
        }
        run.status = status;
        run.updated_at = run.updated_at.max(now);
        self.commit(candidate)
    }

    fn commit(&mut self, candidate: Snapshot) -> Result<(), StoreError> {
        if candidate == self.state {
            return Ok(());
        }
        write_atomic(&self.path, &serde_json::to_vec_pretty(&candidate)?)?;
        self.state = candidate;
        Ok(())
    }
}

fn run_id(occurrence: &Occurrence) -> String {
    // Length-prefix makes arbitrary schedule IDs unambiguous without another hashing dependency.
    format!(
        "{}:{}:{}:{}",
        occurrence.schedule_id.len(),
        occurrence.schedule_id,
        occurrence.timing_revision,
        occurrence.scheduled_for.to_rfc3339()
    )
}

fn sort_runs(runs: &mut [QueuedRun]) {
    runs.sort_by(|a, b| {
        a.occurrence
            .scheduled_for
            .cmp(&b.occurrence.scheduled_for)
            .then(a.run_id.cmp(&b.run_id))
    });
}

fn validate(state: &Snapshot) -> Result<(), StoreError> {
    if state.version != VERSION {
        return Err(StoreError::Version(u64::from(state.version)));
    }
    for (id, entry) in &state.schedules {
        entry.schedule.rule()?;
        if id != &entry.schedule.id {
            return Err(StoreError::InvalidState("schedule key mismatch".into()));
        }
    }
    for (id, run) in &state.runs {
        let expected = run_id(&run.occurrence);
        if id != &run.run_id || *id != expected {
            return Err(StoreError::InvalidState("run identity mismatch".into()));
        }
        if run.status == RunStatus::Pending
            && !state
                .schedules
                .get(&run.occurrence.schedule_id)
                .is_some_and(|entry| {
                    entry.schedule.enabled
                        && entry.schedule.timing_revision == run.occurrence.timing_revision
                })
        {
            return Err(StoreError::InvalidState(
                "pending run has no active matching schedule".into(),
            ));
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            "account file has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".schedules-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    } else {
        // Rename is the commit point. A directory-sync failure must not leave memory on the
        // old snapshot after the new one became visible. Some filesystems cannot sync dirs.
        #[cfg(unix)]
        if let Err(error) = fs::File::open(parent).and_then(|dir| dir.sync_all()) {
            log::warn!("schedule directory sync failed after commit: {error}");
        }
    }
    result
}

#[cfg(test)]
mod tests;
