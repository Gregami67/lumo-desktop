use super::*;
use crate::schedules::{MissedRunPolicy, Schedule, Timing};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::mpsc;

fn account(id: &str) -> AccountId {
    AccountId::parse(id).unwrap()
}
fn at(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}
fn path(root: &Path, id: &AccountId) -> PathBuf {
    root.join("accounts")
        .join(id.as_str())
        .join("schedules.json")
}
fn seed(root: &Path, id: &AccountId) {
    let mut store = ScheduleStore::open(path(root, id)).unwrap();
    store
        .replace_schedules(
            vec![Schedule {
                id: "daily".into(),
                timing_revision: 1,
                enabled: true,
                timing: Timing::Daily {
                    time: "09:00".into(),
                    timezone: "UTC".into(),
                },
                missed_run_policy: MissedRunPolicy::CatchUp,
            }],
            at("2026-10-15T08:00:00Z"),
        )
        .unwrap();
}
fn core(root: &Path) -> CoordinatorCore {
    let root = root.to_path_buf();
    CoordinatorCore::new(move |id| ScheduleStore::open(path(&root, id)).map_err(|e| e.to_string()))
}

#[test]
fn startup_queues_cached_due_work_and_repeated_checks_do_not_replay_it() {
    let dir = tempfile::tempdir().unwrap();
    let id = account("alice");
    seed(dir.path(), &id);
    let mut coordinator = core(dir.path());
    assert_eq!(
        coordinator
            .check(Some(id.clone()), at("2026-10-15T09:00:00Z"))
            .unwrap(),
        1
    );
    assert_eq!(
        coordinator
            .check(Some(id.clone()), at("2026-10-15T09:15:00Z"))
            .unwrap(),
        0
    );
    assert_eq!(
        ScheduleStore::open(path(dir.path(), &id))
            .unwrap()
            .pending_runs()
            .len(),
        1
    );
    drop(coordinator);
    assert_eq!(
        core(dir.path())
            .check(Some(id), at("2026-10-15T09:30:00Z"))
            .unwrap(),
        0
    );
}

#[test]
fn long_sleep_uses_the_planner_to_catch_up_and_clock_rollback_does_not_replay() {
    let dir = tempfile::tempdir().unwrap();
    let id = account("alice");
    seed(dir.path(), &id);
    let mut coordinator = core(dir.path());
    assert_eq!(
        coordinator
            .check(Some(id.clone()), at("2026-10-15T09:00:00Z"))
            .unwrap(),
        1
    );
    assert_eq!(
        coordinator
            .check(Some(id.clone()), at("2026-10-18T10:00:00Z"))
            .unwrap(),
        3
    );
    assert_eq!(
        coordinator
            .check(Some(id.clone()), at("2026-10-16T10:00:00Z"))
            .unwrap(),
        0
    );
    assert_eq!(
        ScheduleStore::open(path(dir.path(), &id))
            .unwrap()
            .pending_runs()
            .len(),
        4
    );
}

#[test]
fn account_switch_and_logout_never_enqueue_for_the_outgoing_account() {
    let dir = tempfile::tempdir().unwrap();
    let alice = account("alice");
    let bob = account("bob");
    seed(dir.path(), &alice);
    seed(dir.path(), &bob);
    let mut coordinator = core(dir.path());
    coordinator
        .check(Some(alice.clone()), at("2026-10-15T08:30:00Z"))
        .unwrap();
    assert_eq!(
        coordinator
            .check(Some(bob.clone()), at("2026-10-15T09:00:00Z"))
            .unwrap(),
        1
    );
    assert_eq!(
        coordinator.check(None, at("2026-10-16T10:00:00Z")).unwrap(),
        0
    );
    assert!(coordinator.loaded.is_none());
    assert!(ScheduleStore::open(path(dir.path(), &alice))
        .unwrap()
        .pending_runs()
        .is_empty());
    assert_eq!(
        ScheduleStore::open(path(dir.path(), &bob))
            .unwrap()
            .pending_runs()
            .len(),
        1
    );
    assert_eq!(
        coordinator
            .check(Some(alice), at("2026-10-16T10:00:00Z"))
            .unwrap(),
        2
    );
}

#[test]
fn corrupt_store_is_not_overwritten_and_loading_is_retried_after_repair() {
    let dir = tempfile::tempdir().unwrap();
    let id = account("alice");
    seed(dir.path(), &id);
    let file = path(dir.path(), &id);
    let original = std::fs::read(&file).unwrap();
    std::fs::write(&file, b"{broken").unwrap();
    let mut coordinator = core(dir.path());
    assert!(coordinator
        .check(Some(id.clone()), at("2026-10-15T09:00:00Z"))
        .is_err());
    assert!(coordinator.loaded.is_none());
    assert_eq!(std::fs::read(&file).unwrap(), b"{broken");
    std::fs::write(&file, original).unwrap();
    assert_eq!(
        coordinator
            .check(Some(id), at("2026-10-15T09:00:00Z"))
            .unwrap(),
        1
    );
}

#[test]
fn failed_incoming_load_drops_the_previous_account_store() {
    let dir = tempfile::tempdir().unwrap();
    let alice = account("alice");
    let bob = account("bob");
    seed(dir.path(), &alice);
    seed(dir.path(), &bob);
    std::fs::write(path(dir.path(), &bob), b"invalid").unwrap();
    let mut coordinator = core(dir.path());
    coordinator
        .check(Some(alice.clone()), at("2026-10-15T08:30:00Z"))
        .unwrap();
    assert!(coordinator
        .check(Some(bob), at("2026-10-15T09:00:00Z"))
        .is_err());
    assert!(coordinator.loaded.is_none());
    assert!(ScheduleStore::open(path(dir.path(), &alice))
        .unwrap()
        .pending_runs()
        .is_empty());
}

#[test]
fn signed_out_and_empty_account_checks_do_not_create_files() {
    let dir = tempfile::tempdir().unwrap();
    let id = account("alice");
    let mut coordinator = core(dir.path());
    assert_eq!(coordinator.check(None, Utc::now()).unwrap(), 0);
    assert_eq!(coordinator.check(Some(id.clone()), Utc::now()).unwrap(), 0);
    assert!(!path(dir.path(), &id).exists());
}

#[tokio::test(start_paused = true)]
async fn loop_checks_immediately_at_the_wait_cap_and_on_account_changes_then_stops() {
    let active = ActiveAccount::default();
    let loads = Arc::new(AtomicUsize::new(0));
    let counted_loads = loads.clone();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let coordinator = CoordinatorCore::new(move |id| {
        counted_loads.fetch_add(1, Ordering::SeqCst);
        ScheduleStore::open(path(&root, id)).map_err(|e| e.to_string())
    });
    let (stop, receiver) = watch::channel(false);
    let (_requests, inbox) = mpsc::channel(64);
    let (checked, mut checks) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(
        coordinator,
        active.subscribe(),
        receiver,
        inbox,
        MAX_WAIT,
        move |result| {
            checked.send(result).unwrap();
        },
        |_| {},
    ));
    assert_eq!(checks.recv().await.unwrap().unwrap(), 0);
    assert_eq!(loads.load(Ordering::SeqCst), 0);
    active.set(Some(account("alice")));
    assert_eq!(checks.recv().await.unwrap().unwrap(), 0);
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    tokio::time::advance(MAX_WAIT).await;
    assert_eq!(checks.recv().await.unwrap().unwrap(), 0);
    assert_eq!(loads.load(Ordering::SeqCst), 1); // Existing store is retained between ticks.
    active.set(Some(account("bob")));
    assert_eq!(checks.recv().await.unwrap().unwrap(), 0);
    assert_eq!(loads.load(Ordering::SeqCst), 2);
    active.set(None);
    assert_eq!(checks.recv().await.unwrap().unwrap(), 0);
    stop.send_replace(true);
    task.await.unwrap();
    assert!(checks.recv().await.is_none());
}

fn connect(core: &mut CoordinatorCore, context: &Option<AccountId>) -> String {
    match core
        .handle(
            context,
            Operation::Connect {
                account: context.clone().unwrap(),
                protocol_version: PROTOCOL_VERSION,
            },
            at("2026-10-15T08:00:00Z"),
        )
        .unwrap()
    {
        Response::Connected(connection) => connection.connection_id,
        _ => panic!("expected a connection"),
    }
}

fn pending(
    core: &mut CoordinatorCore,
    context: &Option<AccountId>,
    connection: &str,
) -> Vec<QueuedRun> {
    match core
        .handle(
            context,
            Operation::Pending {
                connection_id: connection.into(),
            },
            Utc::now(),
        )
        .unwrap()
    {
        Response::Runs(runs) => runs,
        _ => panic!("expected runs"),
    }
}

#[test]
fn bridge_round_trip_syncs_lists_accepts_and_reconciles_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = core(dir.path());
    let context = Some(account("alice"));
    let connection = connect(&mut core, &context);
    let definition = Schedule {
        id: "once".into(),
        timing_revision: 1,
        enabled: true,
        timing: Timing::Once {
            at: at("2026-10-15T09:00:00Z"),
        },
        missed_run_policy: MissedRunPolicy::RunOnce,
    };
    core.handle(
        &context,
        Operation::Replace {
            connection_id: connection.clone(),
            schedules: vec![definition],
        },
        at("2026-10-15T08:00:00Z"),
    )
    .unwrap();
    assert!(pending(&mut core, &context, &connection).is_empty());
    assert_eq!(
        core.check(context.clone(), at("2026-10-15T09:00:00Z"))
            .unwrap(),
        1
    );
    let runs = pending(&mut core, &context, &connection);
    assert_eq!(runs.len(), 1);
    assert_eq!(pending(&mut core, &context, &connection), runs);
    assert_eq!(core.work_notice().unwrap().connection_id, connection);
    core.handle(
        &context,
        Operation::Report {
            connection_id: connection.clone(),
            run_id: runs[0].run_id.clone(),
            status: RunStatus::Accepted,
        },
        Utc::now(),
    )
    .unwrap();
    assert!(core.work_notice().is_none());
    let second = connect(&mut core, &context);
    match core
        .handle(
            &context,
            Operation::Unsettled {
                connection_id: second.clone(),
            },
            Utc::now(),
        )
        .unwrap()
    {
        Response::Runs(runs) => assert_eq!(runs[0].status, RunStatus::Accepted),
        _ => panic!("expected unsettled run"),
    }
    core.handle(
        &context,
        Operation::Report {
            connection_id: second,
            run_id: runs[0].run_id.clone(),
            status: RunStatus::Succeeded,
        },
        Utc::now(),
    )
    .unwrap();
}

#[test]
fn reconnect_invalidates_all_old_read_and_write_requests() {
    let dir = tempfile::tempdir().unwrap();
    let context = Some(account("alice"));
    seed(dir.path(), context.as_ref().unwrap());
    let mut core = core(dir.path());
    let first = connect(&mut core, &context);
    core.check(context.clone(), at("2026-10-15T09:00:00Z"))
        .unwrap();
    let run = pending(&mut core, &context, &first).remove(0);
    let second = connect(&mut core, &context);
    assert_ne!(first, second);
    for operation in [
        Operation::RunNow {
            connection_id: first.clone(),
            schedule_id: "daily".into(),
        },
        Operation::Pending {
            connection_id: first.clone(),
        },
        Operation::Unsettled {
            connection_id: first.clone(),
        },
        Operation::Replace {
            connection_id: first.clone(),
            schedules: vec![],
        },
        Operation::Report {
            connection_id: first,
            run_id: run.run_id,
            status: RunStatus::Accepted,
        },
    ] {
        assert!(core.handle(&context, operation, Utc::now()).is_err());
    }
    assert_eq!(pending(&mut core, &context, &second).len(), 1);
}

#[test]
fn logout_and_observed_account_switches_invalidate_connections() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = core(dir.path());
    let active = ActiveAccount::default();
    active.set(Some(account("alice")));
    let changes = active.subscribe();
    let connection = connect(&mut core, &changes.borrow().clone());
    active.set(Some(account("bob")));
    assert!(core
        .handle(
            &changes.borrow().clone(),
            Operation::Pending {
                connection_id: connection.clone()
            },
            Utc::now(),
        )
        .is_err());
    active.set(Some(account("alice")));
    assert!(core
        .handle(
            &changes.borrow().clone(),
            Operation::Pending {
                connection_id: connection.clone()
            },
            Utc::now()
        )
        .is_err());
    let _ = connect(&mut core, &changes.borrow().clone());
    active.set(None);
    assert!(core
        .handle(
            &changes.borrow().clone(),
            Operation::Pending {
                connection_id: connection
            },
            Utc::now()
        )
        .is_err());
    assert!(core.connection.is_none());
}

#[test]
fn connect_rejects_wrong_account_and_unsupported_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = core(dir.path());
    let context = Some(account("alice"));
    for operation in [
        Operation::Connect {
            account: account("bob"),
            protocol_version: PROTOCOL_VERSION,
        },
        Operation::Connect {
            account: account("alice"),
            protocol_version: 99,
        },
    ] {
        assert!(core.handle(&context, operation, Utc::now()).is_err());
    }
    assert!(core.connection.is_none());
    assert!(!path(dir.path(), context.as_ref().unwrap()).exists());
}

#[tokio::test]
async fn actor_serves_requests_and_notifies_only_after_work_is_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let active = ActiveAccount::default();
    let alice = account("alice");
    active.set(Some(alice.clone()));
    let file = path(dir.path(), &alice);
    let now = Utc::now();
    let mut store = ScheduleStore::open(file.clone()).unwrap();
    store
        .replace_schedules(
            vec![Schedule {
                id: "once".into(),
                timing_revision: 1,
                enabled: true,
                timing: Timing::Once {
                    at: now - chrono::Duration::hours(1),
                },
                missed_run_policy: MissedRunPolicy::CatchUp,
            }],
            now - chrono::Duration::hours(2),
        )
        .unwrap();
    drop(store);
    let (stop, receiver) = watch::channel(false);
    let (requests, inbox) = mpsc::channel(64);
    let coordinator = ScheduleCoordinator {
        stop,
        requests,
        accounts: active.subscribe(),
    };
    let (notified, mut notices) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(
        core(dir.path()),
        active.subscribe(),
        receiver,
        inbox,
        MAX_WAIT,
        |_| {},
        move |notice| {
            assert_eq!(
                ScheduleStore::open(file.clone())
                    .unwrap()
                    .pending_runs()
                    .len(),
                1
            );
            notified.send(notice).unwrap();
        },
    ));
    let connection = match coordinator
        .request(Operation::Connect {
            account: alice,
            protocol_version: PROTOCOL_VERSION,
        })
        .await
        .unwrap()
    {
        Response::Connected(connection) => connection,
        _ => panic!("expected connection"),
    };
    assert_eq!(
        notices.recv().await.unwrap().connection_id,
        connection.connection_id
    );
    match coordinator
        .request(Operation::Pending {
            connection_id: connection.connection_id,
        })
        .await
        .unwrap()
    {
        Response::Runs(runs) => assert_eq!(runs.len(), 1),
        _ => panic!("expected pending runs"),
    }
    coordinator.shutdown();
    task.await.unwrap();
    assert!(coordinator
        .request(Operation::Connect {
            account: account("alice"),
            protocol_version: PROTOCOL_VERSION
        })
        .await
        .is_err());
}

#[tokio::test]
async fn queued_connect_for_a_different_account_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let active = ActiveAccount::default();
    active.set(Some(account("alice")));
    let accounts = active.subscribe();
    let (stop, receiver) = watch::channel(false);
    let (requests, inbox) = mpsc::channel(64);
    let (reply, response) = oneshot::channel();
    requests
        .send(Request {
            context: accounts.borrow().clone(),
            operation: Operation::Connect {
                account: account("alice"),
                protocol_version: PROTOCOL_VERSION,
            },
            reply,
        })
        .await
        .unwrap();
    active.set(Some(account("bob")));
    let task = tokio::spawn(run(
        core(dir.path()),
        accounts,
        receiver,
        inbox,
        MAX_WAIT,
        |_| {},
        |_| {
            panic!("a rejected connection cannot receive a work notice");
        },
    ));
    assert!(response.await.unwrap().is_err());
    stop.send_replace(true);
    task.await.unwrap();
}

#[test]
fn wakeup_targets_due_time_with_a_cap_and_rechecks_overdue_work_immediately() {
    let now = at("2026-10-15T08:00:00Z");
    assert_eq!(wait_duration(None, now, MAX_WAIT), MAX_WAIT);
    assert_eq!(
        wait_duration(Some(now + chrono::Duration::hours(1)), now, MAX_WAIT),
        MAX_WAIT
    );
    assert_eq!(
        wait_duration(
            Some(now + chrono::Duration::milliseconds(250)),
            now,
            MAX_WAIT
        ),
        Duration::from_millis(250)
    );
    assert_eq!(wait_duration(Some(now), now, MAX_WAIT), Duration::ZERO);
    // Forward clock jumps / resume: the occurrence is overdue, not pushed to tomorrow.
    assert_eq!(
        wait_duration(Some(now - chrono::Duration::hours(1)), now, MAX_WAIT),
        Duration::ZERO
    );
}

#[test]
fn waking_before_wall_clock_due_time_does_not_queue_and_rearms_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let id = account("alice");
    seed(dir.path(), &id);
    let mut coordinator = core(dir.path());
    coordinator
        .check(Some(id.clone()), at("2026-10-15T08:59:59Z"))
        .unwrap();
    let next = coordinator.next_due().unwrap();
    assert_eq!(next, Some(at("2026-10-15T09:00:00Z")));
    // Simulate a backward wall-clock jump while a monotonic timer was asleep.
    assert_eq!(
        coordinator
            .check(Some(id.clone()), at("2026-10-15T08:59:50Z"))
            .unwrap(),
        0
    );
    assert_eq!(coordinator.next_due().unwrap(), next);
    assert_eq!(
        wait_duration(next, at("2026-10-15T08:59:50Z"), MAX_WAIT),
        Duration::from_secs(10)
    );
    assert_eq!(
        coordinator
            .check(Some(id.clone()), at("2026-10-15T09:00:00Z"))
            .unwrap(),
        1
    );
    assert_eq!(
        coordinator.next_due().unwrap(),
        Some(at("2026-10-16T09:00:00Z"))
    );
    assert_eq!(
        coordinator
            .check(Some(id), at("2026-10-15T08:00:00Z"))
            .unwrap(),
        0
    );
    assert_eq!(
        coordinator.next_due().unwrap(),
        Some(at("2026-10-16T09:00:00Z"))
    );
}

#[tokio::test]
async fn replacing_a_far_future_schedule_wakes_at_the_new_due_time_before_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let active = ActiveAccount::default();
    active.set(Some(account("alice")));
    let (stop, receiver) = watch::channel(false);
    let (requests, inbox) = mpsc::channel(64);
    let coordinator = ScheduleCoordinator {
        stop,
        requests,
        accounts: active.subscribe(),
    };
    let (notified, mut notices) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(
        core(dir.path()),
        active.subscribe(),
        receiver,
        inbox,
        MAX_WAIT,
        |_| {},
        move |notice| {
            let _ = notified.send(notice);
        },
    ));
    let connection = match coordinator
        .request(Operation::Connect {
            account: account("alice"),
            protocol_version: PROTOCOL_VERSION,
        })
        .await
        .unwrap()
    {
        Response::Connected(connection) => connection.connection_id,
        _ => panic!("expected connection"),
    };
    let mut definition = Schedule {
        id: "once".into(),
        timing_revision: 1,
        enabled: true,
        timing: Timing::Once {
            at: Utc::now() + chrono::Duration::hours(1),
        },
        missed_run_policy: MissedRunPolicy::RunOnce,
    };
    coordinator
        .request(Operation::Replace {
            connection_id: connection.clone(),
            schedules: vec![definition.clone()],
        })
        .await
        .unwrap();
    let due = Utc::now() + chrono::Duration::milliseconds(700);
    definition.timing_revision = 2;
    definition.timing = Timing::Once { at: due };
    coordinator
        .request(Operation::Replace {
            connection_id: connection.clone(),
            schedules: vec![definition],
        })
        .await
        .unwrap();
    // A stale client polling faster than the wait cap must not postpone the due check.
    let mut rejections = 0;
    let notice = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            tokio::select! {
                notice = notices.recv() => break notice,
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    assert!(coordinator.request(Operation::Pending {
                        connection_id: "expired-connection".into(),
                    }).await.is_err());
                    rejections += 1;
                }
            }
        }
    })
    .await;
    coordinator.shutdown();
    task.await.unwrap();
    assert!(rejections > 0);
    assert_eq!(notice.unwrap().unwrap().connection_id, connection);
    let store = ScheduleStore::open(path(dir.path(), &account("alice"))).unwrap();
    let runs = store.pending_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].occurrence.scheduled_for, due);
    assert!(runs[0].updated_at >= due);
    assert_eq!(store.next_due().unwrap(), None);
}

#[test]
fn execution_permissions_require_an_unsettled_run_and_are_scoped_to_its_connection() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), &account("alice"));
    let mut core = core(dir.path());
    let account = Some(account("alice"));
    let connection = connect(&mut core, &account);
    core.check(account.clone(), at("2026-10-15T09:00:00Z"))
        .unwrap();
    let run_id = pending(&mut core, &account, &connection)[0].run_id.clone();
    let policy = ExecutionPolicy {
        connector_ids: vec!["echo".into()],
        auto_approve: true,
        read_only_connector_ids: vec![],
    };
    let register = || Operation::RegisterExecutionContext {
        run_id: run_id.clone(),
        policy: policy.clone(),
    };
    assert!(
        core.handle(&account, register(), Utc::now()).is_err(),
        "pending is not accepted"
    );
    core.handle(
        &account,
        Operation::Report {
            connection_id: connection.clone(),
            run_id: run_id.clone(),
            status: RunStatus::Accepted,
        },
        Utc::now(),
    )
    .unwrap();
    let context = match core.handle(&account, register(), Utc::now()).unwrap() {
        Response::ExecutionContextRegistered(context) => context,
        _ => panic!("expected execution context"),
    };
    let authorize = |connector: &str| Operation::AuthorizeTool {
        context: context.clone(),
        connector_id: connector.into(),
    };
    assert!(matches!(
        core.handle(&account, authorize("echo"), Utc::now()),
        Ok(Response::ToolAuthorized(ToolAuthorization {
            auto_approve: true,
            read_only: false,
            ..
        }))
    ));
    assert!(core
        .handle(&account, authorize("bash"), Utc::now())
        .is_err());
    assert!(
        core.handle(
            &account,
            Operation::RegisterExecutionContext {
                run_id: run_id.clone(),
                policy: ExecutionPolicy {
                    connector_ids: vec!["bash".into()],
                    auto_approve: true,
                    read_only_connector_ids: vec![],
                },
            },
            Utc::now()
        )
        .is_err(),
        "registered permissions cannot be widened"
    );
    let connection = connect(&mut core, &account);
    assert!(
        core.handle(&account, authorize("echo"), Utc::now())
            .is_err(),
        "reconnect expires old context"
    );
    assert!(core.executions.is_empty());
    let context = match core.handle(&account, register(), Utc::now()).unwrap() {
        Response::ExecutionContextRegistered(context) => context,
        _ => panic!("expected execution context"),
    };
    core.handle(
        &account,
        Operation::Report {
            connection_id: connection,
            run_id: run_id.clone(),
            status: RunStatus::Succeeded,
        },
        Utc::now(),
    )
    .unwrap();
    assert!(core.executions.is_empty());
    assert!(
        core.handle(
            &account,
            Operation::AuthorizeTool {
                context,
                connector_id: "echo".into(),
            },
            Utc::now()
        )
        .is_err(),
        "settled runs cannot execute"
    );
}

#[test]
fn account_changes_clear_execution_permissions() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), &account("alice"));
    let mut core = core(dir.path());
    let alice = Some(account("alice"));
    let connection = connect(&mut core, &alice);
    core.check(alice.clone(), at("2026-10-15T09:00:00Z"))
        .unwrap();
    let run_id = pending(&mut core, &alice, &connection)[0].run_id.clone();
    core.handle(
        &alice,
        Operation::Report {
            connection_id: connection,
            run_id: run_id.clone(),
            status: RunStatus::Accepted,
        },
        Utc::now(),
    )
    .unwrap();
    core.handle(
        &alice,
        Operation::RegisterExecutionContext {
            run_id,
            policy: ExecutionPolicy {
                connector_ids: vec!["echo".into()],
                auto_approve: false,
                read_only_connector_ids: vec![],
            },
        },
        Utc::now(),
    )
    .unwrap();
    core.observe(&Some(account("bob")));
    assert!(core.executions.is_empty());
    assert!(core.connection.is_none());
}

#[test]
fn manual_request_queues_durable_work_for_the_connected_account() {
    let dir = tempfile::tempdir().unwrap();
    let context = Some(account("alice"));
    seed(dir.path(), context.as_ref().unwrap());
    let mut core = core(dir.path());
    let connection = connect(&mut core, &context);
    let run = match core
        .handle(
            &context,
            Operation::RunNow {
                connection_id: connection.clone(),
                schedule_id: "daily".into(),
            },
            at("2026-10-15T08:30:00Z"),
        )
        .unwrap()
    {
        Response::Queued(run) => run,
        _ => panic!("expected a queued manual run"),
    };
    assert_eq!(core.work_notice().unwrap().connection_id, connection);
    assert_eq!(pending(&mut core, &context, &connection), vec![run.clone()]);
    assert_eq!(
        ScheduleStore::open(path(dir.path(), context.as_ref().unwrap()))
            .unwrap()
            .pending_runs(),
        vec![run]
    );
    assert_eq!(core.check(context, at("2026-10-15T09:00:00Z")).unwrap(), 1);
}

#[test]
fn read_only_permissions_are_per_connector_validated_and_immutable_for_a_run() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), &account("alice"));
    let mut core = core(dir.path());
    let account = Some(account("alice"));
    let connection = connect(&mut core, &account);
    core.check(account.clone(), at("2026-10-15T09:00:00Z"))
        .unwrap();
    let run_id = pending(&mut core, &account, &connection)[0].run_id.clone();
    core.handle(
        &account,
        Operation::Report {
            connection_id: connection,
            run_id: run_id.clone(),
            status: RunStatus::Accepted,
        },
        Utc::now(),
    )
    .unwrap();
    let policy = ExecutionPolicy {
        connector_ids: vec!["mail".into(), "calendar".into()],
        auto_approve: true,
        read_only_connector_ids: vec!["mail".into()],
    };
    let register = |policy| Operation::RegisterExecutionContext {
        run_id: run_id.clone(),
        policy,
    };
    let mut invalid = policy.clone();
    invalid.read_only_connector_ids.push("other".into());
    assert!(core
        .handle(&account, register(invalid), Utc::now())
        .err()
        .unwrap()
        .contains("must be attached"));
    let context = match core
        .handle(&account, register(policy.clone()), Utc::now())
        .unwrap()
    {
        Response::ExecutionContextRegistered(context) => context,
        _ => panic!("expected context"),
    };
    for (connector, read_only) in [("mail", true), ("calendar", false)] {
        let response = core.handle(
            &account,
            Operation::AuthorizeTool {
                context: context.clone(),
                connector_id: connector.into(),
            },
            Utc::now(),
        );
        match response.unwrap() {
            Response::ToolAuthorized(access) => assert_eq!(
                access,
                ToolAuthorization {
                    auto_approve: true,
                    read_only,
                    connector_ids: Some(vec!["mail".into(), "calendar".into()]),
                }
            ),
            _ => panic!("expected tool authorization"),
        }
    }
    let mut widened = policy.clone();
    widened.read_only_connector_ids.clear();
    assert!(core
        .handle(&account, register(widened), Utc::now())
        .is_err());
    // Re-registering the same snapshot remains safe and idempotent.
    assert!(core.handle(&account, register(policy), Utc::now()).is_ok());
}
