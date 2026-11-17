use super::*;
use crate::schedules::{MissedRunPolicy, Timing};

fn at(raw: &str) -> DateTime<Utc> {
    raw.parse().unwrap()
}
fn activation() -> DateTime<Utc> {
    at("2026-10-15T08:00:00Z")
}
fn due() -> DateTime<Utc> {
    at("2026-10-15T09:00:00Z")
}
fn schedule() -> Schedule {
    Schedule {
        id: "summary".into(),
        timing_revision: 1,
        enabled: true,
        timing: Timing::Daily {
            time: "09:00".into(),
            timezone: "UTC".into(),
        },
        missed_run_policy: MissedRunPolicy::CatchUp,
    }
}
fn seed(path: &Path) -> ScheduleStore {
    let mut store = ScheduleStore::open(path.to_path_buf()).unwrap();
    store
        .replace_schedules(vec![schedule()], activation())
        .unwrap();
    store
}

#[test]
fn pending_work_and_checkpoint_survive_restart_together() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedules.json");
    let mut store = seed(&path);
    let added = store.enqueue_due(due()).unwrap();
    assert_eq!(added.len(), 1);
    assert_eq!(store.pending_runs(), added);
    assert_eq!(store.pending_runs(), added); // Reading is not an acknowledgment.
    drop(store);
    let mut reopened = ScheduleStore::open(path).unwrap();
    assert_eq!(reopened.pending_runs(), added);
    assert!(reopened.enqueue_due(due()).unwrap().is_empty());
    assert_eq!(reopened.pending_runs(), added);
}

#[test]
fn acceptance_and_running_state_survive_reload_without_automatic_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedules.json");
    let mut store = seed(&path);
    let run = store.enqueue_due(due()).unwrap().remove(0);
    store
        .report_status(&run.run_id, RunStatus::Accepted, due())
        .unwrap();
    assert!(store.pending_runs().is_empty());
    drop(store);
    let mut store = ScheduleStore::open(path.clone()).unwrap();
    assert_eq!(store.unsettled_runs()[0].status, RunStatus::Accepted);
    store
        .report_status(&run.run_id, RunStatus::Running, due())
        .unwrap();
    let mut store = ScheduleStore::open(path.clone()).unwrap();
    assert_eq!(store.unsettled_runs()[0].status, RunStatus::Running);
    store
        .report_status(&run.run_id, RunStatus::Succeeded, due())
        .unwrap();
    store
        .report_status(&run.run_id, RunStatus::Succeeded, due())
        .unwrap();
    assert!(matches!(
        store.report_status(&run.run_id, RunStatus::Accepted, due()),
        Err(StoreError::InvalidTransition { .. })
    ));
    let mut store = ScheduleStore::open(path).unwrap();
    assert!(store.pending_runs().is_empty());
    assert!(store.unsettled_runs().is_empty());
    assert!(store.enqueue_due(due()).unwrap().is_empty());
    assert_eq!(store.state.runs[&run.run_id].status, RunStatus::Succeeded);
}

#[test]
fn failed_write_leaves_memory_disk_and_checkpoint_unchanged_and_can_be_retried() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedules.json");
    let mut store = seed(&path);
    let before = fs::read(&path).unwrap();
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, b"not a directory").unwrap();
    store.path = blocker.join("schedules.json");
    assert!(store.enqueue_due(due()).is_err());
    assert!(store.pending_runs().is_empty());
    assert_eq!(
        store.state.schedules["summary"].checked_through,
        activation()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    store.path = path;
    assert_eq!(store.enqueue_due(due()).unwrap().len(), 1);
}

#[test]
fn orphaned_partial_temp_file_is_ignored_and_failed_rename_cleans_its_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedules.json");
    let mut store = seed(&path);
    fs::write(dir.path().join(".schedules-crashed.tmp"), b"{partial").unwrap();
    let reopened = ScheduleStore::open(path.clone()).unwrap();
    assert!(reopened.pending_runs().is_empty());
    store.enqueue_due(due()).unwrap();
    assert_eq!(ScheduleStore::open(path).unwrap().pending_runs().len(), 1);
    let target_directory = dir.path().join("directory");
    fs::create_dir(&target_directory).unwrap();
    assert!(write_atomic(&target_directory, b"new state").is_err());
    let temp_files = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(temp_files, 1); // Only the simulated orphan remains.
}

#[test]
fn invalid_or_newer_committed_state_is_preserved_and_never_silently_reset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedules.json");
    for bytes in [b"{broken".as_slice(), br#"{"version":999}"#.as_slice()] {
        fs::write(&path, bytes).unwrap();
        assert!(ScheduleStore::open(path.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    let store = seed(&path.with_file_name("valid.json"));
    let mut state = store.state;
    state.schedules.get_mut("summary").unwrap().schedule.id = "wrong-id".into();
    fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(matches!(
        ScheduleStore::open(path),
        Err(StoreError::InvalidState(_))
    ));
}

#[test]
fn unchanged_snapshot_retains_checkpoint_so_missed_runs_are_recovered_after_sync() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = seed(&dir.path().join("schedules.json"));
    let later = at("2026-10-17T10:00:00Z");
    store.replace_schedules(vec![schedule()], later).unwrap();
    let added = store.enqueue_due(later).unwrap();
    assert_eq!(added.len(), 3);
    assert_eq!(added[0].occurrence.scheduled_for, due());
    assert!(store.enqueue_due(later).unwrap().is_empty());
}

#[test]
fn deleted_disabled_and_revised_schedules_skip_pending_but_preserve_accepted_work() {
    for change in ["delete", "disable", "revise"] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = seed(&dir.path().join("schedules.json"));
        let added = store.enqueue_due(at("2026-10-16T10:00:00Z")).unwrap();
        store
            .report_status(&added[0].run_id, RunStatus::Accepted, due())
            .unwrap();
        let mut next = schedule();
        if change == "disable" {
            next.enabled = false;
        }
        if change == "revise" {
            next.timing_revision = 2;
        }
        store
            .replace_schedules(
                if change == "delete" {
                    vec![]
                } else {
                    vec![next]
                },
                at("2026-10-16T11:00:00Z"),
            )
            .unwrap();
        assert!(store.pending_runs().is_empty());
        assert_eq!(
            store.state.runs[&added[1].run_id].status,
            RunStatus::Skipped
        );
        assert_eq!(store.unsettled_runs().len(), 1);
    }
}

#[test]
fn reenable_and_timing_revision_activate_now_without_replaying_the_disabled_interval() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = seed(&dir.path().join("schedules.json"));
    let mut definition = schedule();
    definition.enabled = false;
    store
        .replace_schedules(vec![definition.clone()], activation())
        .unwrap();
    definition.enabled = true;
    let later = at("2026-10-17T10:00:00Z");
    store
        .replace_schedules(vec![definition.clone()], later)
        .unwrap();
    assert!(store.enqueue_due(later).unwrap().is_empty());
    assert_eq!(
        store.enqueue_due(at("2026-10-18T09:00:00Z")).unwrap().len(),
        1
    );
    definition.timing_revision += 1;
    store
        .replace_schedules(vec![definition], at("2026-10-20T10:00:00Z"))
        .unwrap();
    assert!(store
        .enqueue_due(at("2026-10-20T10:00:00Z"))
        .unwrap()
        .is_empty());
}

#[test]
fn invalid_snapshots_are_rejected_without_partial_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedules.json");
    let mut store = seed(&path);
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        store.replace_schedules(vec![schedule(), schedule()], due()),
        Err(StoreError::DuplicateSchedule(_))
    ));
    let mut changed = schedule();
    changed.timing = Timing::Once { at: due() };
    assert!(matches!(
        store.replace_schedules(vec![changed], due()),
        Err(StoreError::RevisionConflict(_))
    ));
    let mut stale = schedule();
    stale.timing_revision = 0;
    assert!(matches!(
        store.replace_schedules(vec![stale], due()),
        Err(StoreError::RevisionConflict(_))
    ));
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(store.enqueue_due(due()).unwrap().len(), 1);
}

#[test]
fn separate_account_directories_do_not_share_pending_work() {
    let dir = tempfile::tempdir().unwrap();
    let alice = dir.path().join("accounts/alice/schedules.json");
    let bob = dir.path().join("accounts/bob/schedules.json");
    let mut store = seed(&alice);
    store.enqueue_due(due()).unwrap();
    let other = ScheduleStore::open(bob).unwrap();
    assert!(other.pending_runs().is_empty());
    assert_eq!(ScheduleStore::open(alice).unwrap().pending_runs().len(), 1);
}

#[test]
fn reconciliation_can_settle_accepted_work_when_running_notification_was_lost() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedules.json");
    let mut store = seed(&path);
    let run = store.enqueue_due(due()).unwrap().remove(0);
    store
        .report_status(&run.run_id, RunStatus::Accepted, due())
        .unwrap();
    drop(store);
    let mut store = ScheduleStore::open(path).unwrap();
    store
        .report_status(&run.run_id, RunStatus::Succeeded, due())
        .unwrap();
    assert!(store.unsettled_runs().is_empty());
    assert!(matches!(
        store.report_status("missing", RunStatus::Accepted, due()),
        Err(StoreError::UnknownRun(_))
    ));
    assert!(matches!(
        store.report_status(&run.run_id, RunStatus::Running, due()),
        Err(StoreError::InvalidTransition { .. })
    ));
}

#[test]
fn next_due_uses_each_checkpoint_and_ignores_disabled_and_removed_schedules() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = ScheduleStore::open(dir.path().join("schedules.json")).unwrap();
    assert_eq!(store.next_due().unwrap(), None);
    let mut earlier = schedule();
    earlier.id = "earlier".into();
    earlier.timing = Timing::Once {
        at: activation() + chrono::Duration::minutes(5),
    };
    let mut disabled = earlier.clone();
    disabled.id = "disabled".into();
    disabled.enabled = false;
    disabled.timing = Timing::Once {
        at: activation() + chrono::Duration::minutes(1),
    };
    store
        .replace_schedules(vec![schedule(), earlier, disabled], activation())
        .unwrap();
    assert_eq!(
        store.next_due().unwrap(),
        Some(activation() + chrono::Duration::minutes(5))
    );
    // Queueing that occurrence exhausts the one-off and reveals the daily occurrence.
    store
        .enqueue_due(activation() + chrono::Duration::minutes(5))
        .unwrap();
    assert_eq!(store.next_due().unwrap(), Some(due()));
    store.replace_schedules(vec![], due()).unwrap();
    assert_eq!(store.next_due().unwrap(), None);
}

#[test]
fn local_timezone_changes_keep_the_shared_revision_and_cancel_only_pending_old_zone_work() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedules.json");
    let mut store = seed(&path);
    let pending = store.enqueue_due(due()).unwrap().remove(0);
    let next_day = at("2026-10-16T09:00:00Z");
    let accepted = store.enqueue_due(next_day).unwrap().remove(0);
    store
        .report_status(&accepted.run_id, RunStatus::Accepted, next_day)
        .unwrap();
    let mut local = schedule();
    local.timing = Timing::Daily {
        time: "09:00".into(),
        timezone: "Asia/Taipei".into(),
    };
    store
        .replace_schedules(vec![local.clone()], next_day)
        .unwrap();
    assert!(store.pending_runs().is_empty());
    assert_eq!(store.state.runs[&pending.run_id].status, RunStatus::Skipped);
    assert_eq!(store.unsettled_runs()[0].run_id, accepted.run_id);
    assert!(store.enqueue_due(next_day).unwrap().is_empty());
    let due_in_new_zone = at("2026-10-17T01:00:00Z");
    let new_run = store.enqueue_due(due_in_new_zone).unwrap().remove(0);
    assert_eq!(new_run.occurrence.timing_revision, 1);
    assert_eq!(new_run.occurrence.scheduled_for, due_in_new_zone);
    // Changing the wall-clock time itself still requires a new shared revision.
    local.timing = Timing::Daily {
        time: "10:00".into(),
        timezone: "Asia/Taipei".into(),
    };
    assert!(matches!(
        store.replace_schedules(vec![local], due_in_new_zone),
        Err(StoreError::RevisionConflict(_))
    ));
}

#[test]
fn manual_runs_survive_restart_and_do_not_consume_the_timed_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedules.json");
    let mut store = seed(&path);
    let first = store
        .enqueue_manual("summary", at("2026-10-15T08:30:00.123456789Z"))
        .unwrap();
    let second = store
        .enqueue_manual("summary", at("2026-10-15T08:30:00.123456790Z"))
        .unwrap();
    assert_eq!(first.run_id, run_id(&first.occurrence));
    assert_eq!(second.run_id, run_id(&second.occurrence));
    assert_ne!(first.run_id, second.run_id);
    assert_eq!(first.occurrence.timing_revision, 1);
    assert_eq!(first.status, RunStatus::Pending);
    assert!(first.manual);
    assert!(second.manual);
    let mut store = ScheduleStore::open(path).unwrap();
    assert_eq!(store.pending_runs().len(), 2);
    assert!(store.pending_runs().iter().all(|run| run.manual));
    let timed = store.enqueue_due(due()).unwrap();
    assert_eq!(timed.len(), 1);
    assert!(!timed[0].manual);
    assert_ne!(timed[0].run_id, first.run_id);
    assert_eq!(store.pending_runs().len(), 3);
    assert!(store.enqueue_due(due()).unwrap().is_empty());
}

#[test]
fn manual_runs_require_an_enabled_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = seed(&dir.path().join("schedules.json"));
    assert!(store.enqueue_manual("missing", due()).is_err());
    let mut disabled = schedule();
    disabled.enabled = false;
    store
        .replace_schedules(vec![disabled], activation())
        .unwrap();
    assert!(store.enqueue_manual("summary", due()).is_err());
    assert!(store.pending_runs().is_empty());
}

#[test]
fn manual_timestamp_collisions_return_the_existing_run_without_resetting_its_status() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedules.json");
    let mut store = seed(&path);
    let now = at("2026-10-15T08:30:00.123456789Z");
    let run = store.enqueue_manual("summary", now).unwrap();
    for status in [
        RunStatus::Pending,
        RunStatus::Accepted,
        RunStatus::Running,
        RunStatus::Succeeded,
    ] {
        store.report_status(&run.run_id, status, now).unwrap();
        let existing = store.state.runs[&run.run_id].clone();
        let before = fs::read(&path).unwrap();
        assert_eq!(store.enqueue_manual("summary", now).unwrap(), existing);
        assert_eq!(store.state.runs.len(), 1);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    let mut reopened = ScheduleStore::open(path).unwrap();
    assert_eq!(
        reopened.enqueue_manual("summary", now).unwrap().status,
        RunStatus::Succeeded
    );
}

#[test]
fn manual_and_timed_requests_for_the_same_occurrence_share_a_run() {
    for manual_first in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = seed(&dir.path().join("schedules.json"));
        let run = if manual_first {
            let run = store.enqueue_manual("summary", due()).unwrap();
            assert!(store.enqueue_due(due()).unwrap().is_empty());
            run
        } else {
            let run = store.enqueue_due(due()).unwrap().remove(0);
            assert_eq!(store.enqueue_manual("summary", due()).unwrap(), run);
            run
        };
        assert_eq!(run.manual, manual_first);
        assert_eq!(store.pending_runs(), vec![run]);
        assert!(store.enqueue_due(due()).unwrap().is_empty());
        assert_eq!(store.next_due().unwrap(), Some(at("2026-10-16T09:00:00Z")));
    }
}

#[test]
fn legacy_runs_default_to_automatic_and_not_late() {
    let run: QueuedRun = serde_json::from_value(serde_json::json!({
        "run_id": "legacy",
        "occurrence": {
            "schedule_id": "summary", "timing_revision": 1,
            "scheduled_for": "2026-10-15T09:00:00Z"
        },
        "status": "pending", "updated_at": "2026-10-15T09:00:00Z"
    }))
    .unwrap();
    assert!(!run.manual);
    assert!(!run.late);
}

#[test]
fn late_classification_uses_a_strict_one_minute_grace_for_both_policies() {
    for policy in [MissedRunPolicy::CatchUp, MissedRunPolicy::RunOnce] {
        for (delay_ms, expected_late) in [(0, false), (1, false), (60_000, false), (60_001, true)] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("schedules.json");
            let mut store = seed(&path);
            let mut definition = schedule();
            definition.missed_run_policy = policy;
            store
                .replace_schedules(vec![definition], activation())
                .unwrap();
            let now = due() + Duration::milliseconds(delay_ms);
            let run = store.enqueue_due(now).unwrap().remove(0);
            assert_eq!(run.late, expected_late, "{policy:?}, delay={delay_ms}");
            assert!(!run.manual);
            // Later status updates and retries must not reclassify an on-time run.
            let later = now + Duration::hours(1);
            store
                .report_status(&run.run_id, RunStatus::Accepted, later)
                .unwrap();
            store
                .report_status(&run.run_id, RunStatus::Running, later)
                .unwrap();
            let mut reopened = ScheduleStore::open(path).unwrap();
            let persisted = &reopened.unsettled_runs()[0];
            assert_eq!(persisted.late, expected_late);
            assert_eq!(persisted.occurrence.scheduled_for, due());
            assert_eq!(
                reopened.enqueue_manual("summary", due()).unwrap().late,
                expected_late
            );
        }
    }
}

#[test]
fn catch_up_classifies_each_occurrence_and_manual_runs_are_never_late() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedules.json");
    let mut store = seed(&path);
    let now = at("2026-10-17T09:00:30Z");
    let runs = store.enqueue_due(now).unwrap();
    assert_eq!(
        runs.iter().map(|run| run.late).collect::<Vec<_>>(),
        vec![true, true, false]
    );
    let manual = store.enqueue_manual("summary", now).unwrap();
    assert!(manual.manual);
    assert!(!manual.late);
    let reopened = ScheduleStore::open(path).unwrap();
    assert_eq!(reopened.pending_runs().last().unwrap(), &manual);
}
