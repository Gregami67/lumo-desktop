//! Coordinates local scheduling for the active account, including due-run checks,
//! webview requests, and notifications when queued work is available.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{sleep_until, Instant};

use super::bridge::{
    Connection, ExecutionPolicy, ScheduleToolContext, ToolAuthorization, WorkAvailable,
    PROTOCOL_VERSION,
};
use super::store::{QueuedRun, RunStatus, ScheduleStore};
use super::Schedule;
use crate::account::{AccountId, ActiveAccount};

const MAX_WAIT: Duration = Duration::from_secs(30);

pub(super) enum Operation {
    RegisterExecutionContext {
        run_id: String,
        policy: ExecutionPolicy,
    },
    AuthorizeTool {
        context: ScheduleToolContext,
        connector_id: String,
    },
    Connect {
        account: AccountId,
        protocol_version: u32,
    },
    Replace {
        connection_id: String,
        schedules: Vec<Schedule>,
    },
    RunNow {
        connection_id: String,
        schedule_id: String,
    },
    Pending {
        connection_id: String,
    },
    Unsettled {
        connection_id: String,
    },
    Report {
        connection_id: String,
        run_id: String,
        status: RunStatus,
    },
}

pub(super) enum Response {
    ExecutionContextRegistered(ScheduleToolContext),
    ToolAuthorized(ToolAuthorization),
    Connected(Connection),
    Runs(Vec<QueuedRun>),
    Queued(QueuedRun),
    Done,
    Checked(usize),
}

struct Request {
    operation: Operation,
    context: Option<AccountId>,
    reply: oneshot::Sender<Result<Response, String>>,
}

pub struct ScheduleCoordinator {
    stop: watch::Sender<bool>,
    requests: mpsc::Sender<Request>,
    accounts: watch::Receiver<Option<AccountId>>,
}

impl ScheduleCoordinator {
    pub fn start(app: &AppHandle) -> Self {
        let accounts = app.state::<ActiveAccount>().subscribe();
        let (stop, receiver) = watch::channel(false);
        let (requests, inbox) = mpsc::channel(64);
        let loader = app.clone();
        let events = app.clone();
        let core =
            CoordinatorCore::new(move |account| ScheduleStore::open_for_account(&loader, account));
        tauri::async_runtime::spawn(run(
            core,
            accounts.clone(),
            receiver,
            inbox,
            MAX_WAIT,
            |result| match result {
                Ok(count) if count > 0 => log::info!("scheduler queued {count} run(s)"),
                Ok(_) => {}
                Err(error) => log::warn!("scheduler check failed; will retry: {error}"),
            },
            move |notice| {
                if let Err(error) = events.emit_to(
                    crate::main_window::LABEL,
                    "schedules:work_available",
                    notice,
                ) {
                    log::warn!("could not notify web of scheduled work: {error}");
                }
            },
        ));
        Self {
            stop,
            requests,
            accounts,
        }
    }

    pub(super) async fn request(&self, operation: Operation) -> Result<Response, String> {
        if *self.stop.borrow() {
            return Err("scheduler is stopped".into());
        }
        let (reply, response) = oneshot::channel();
        let context = self.accounts.borrow().clone();
        self.requests
            .send(Request {
                operation,
                context,
                reply,
            })
            .await
            .map_err(|_| "scheduler is stopped")?;
        response
            .await
            .map_err(|_| "scheduler stopped before replying".to_string())?
    }

    pub(crate) async fn register_execution_context(
        &self,
        run_id: String,
        connector_ids: Vec<String>,
        auto_approve: bool,
        read_only_connector_ids: Vec<String>,
    ) -> Result<ScheduleToolContext, String> {
        match self
            .request(Operation::RegisterExecutionContext {
                run_id,
                policy: ExecutionPolicy {
                    connector_ids,
                    auto_approve,
                    read_only_connector_ids,
                },
            })
            .await?
        {
            Response::ExecutionContextRegistered(context) => Ok(context),
            _ => Err("unexpected scheduler response".into()),
        }
    }

    pub(crate) async fn authorize_tool(
        &self,
        context: ScheduleToolContext,
        connector_id: String,
    ) -> Result<ToolAuthorization, String> {
        match self
            .request(Operation::AuthorizeTool {
                context,
                connector_id,
            })
            .await?
        {
            Response::ToolAuthorized(auto_approve) => Ok(auto_approve),
            _ => Err("unexpected scheduler response".into()),
        }
    }

    pub fn shutdown(&self) {
        self.stop.send_replace(true);
    }
}

impl Drop for ScheduleCoordinator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

type OpenStore = Box<dyn Fn(&AccountId) -> Result<ScheduleStore, String> + Send>;

struct CoordinatorCore {
    open: OpenStore,
    loaded: Option<(AccountId, ScheduleStore)>,
    connection: Option<Connection>,
    executions: HashMap<String, ExecutionPolicy>,
}

impl CoordinatorCore {
    fn new(open: impl Fn(&AccountId) -> Result<ScheduleStore, String> + Send + 'static) -> Self {
        Self {
            open: Box::new(open),
            loaded: None,
            connection: None,
            executions: HashMap::new(),
        }
    }

    fn observe(&mut self, account: &Option<AccountId>) {
        if self.loaded.as_ref().map(|(id, _)| id) != account.as_ref() {
            self.loaded = None;
            self.connection = None;
            self.executions.clear();
        }
    }

    fn load(&mut self, account: &AccountId) -> Result<&mut ScheduleStore, String> {
        if self.loaded.as_ref().map(|(id, _)| id) != Some(account) {
            // Never fall back to the previous account's store on an error; retry next tick.
            self.loaded = None;
            self.loaded = Some((account.clone(), (self.open)(account)?));
        }
        self.loaded
            .as_mut()
            .map(|(_, store)| store)
            .ok_or_else(|| "account store is not loaded".to_string())
    }

    fn check(&mut self, account: Option<AccountId>, now: DateTime<Utc>) -> Result<usize, String> {
        let Some(account) = account else {
            self.loaded = None;
            self.connection = None;
            self.executions.clear();
            return Ok(0);
        };
        self.load(&account)?
            .enqueue_due(now)
            .map(|runs| runs.len())
            .map_err(|e| e.to_string())
    }

    fn handle(
        &mut self,
        snapshot: &Option<AccountId>,
        operation: Operation,
        now: DateTime<Utc>,
    ) -> Result<Response, String> {
        self.observe(snapshot);
        let account = snapshot.as_ref().ok_or("no account is signed in")?;
        if let Operation::Connect {
            account: expected,
            protocol_version,
        } = operation
        {
            if protocol_version != PROTOCOL_VERSION {
                return Err("unsupported schedule bridge protocol".into());
            }
            if expected != *account {
                return Err("schedule account does not match the active account".into());
            }
            self.load(account)?;
            let connection = Connection {
                connection_id: uuid::Uuid::new_v4().to_string(),
                protocol_version: PROTOCOL_VERSION,
            };
            self.executions.clear();
            self.connection = Some(connection.clone());
            return Ok(Response::Connected(connection));
        }
        if let Operation::RegisterExecutionContext { run_id, policy } = operation {
            if policy
                .read_only_connector_ids
                .iter()
                .any(|id| !policy.connector_ids.contains(id))
            {
                return Err("read-only connectors must be attached to this scheduled task".into());
            }
            let connection_id = self
                .connection
                .as_ref()
                .ok_or("schedule connection expired; reconnect")?
                .connection_id
                .clone();
            if !self.load(account)?.is_unsettled(&run_id) {
                return Err("schedule run is not accepted or running".into());
            }
            if self
                .executions
                .get(&run_id)
                .is_some_and(|existing| existing != &policy)
            {
                return Err("schedule execution permissions already registered".into());
            }
            self.executions.insert(run_id.clone(), policy);
            return Ok(Response::ExecutionContextRegistered(ScheduleToolContext {
                connection_id,
                run_id,
            }));
        }
        let id = match &operation {
            Operation::AuthorizeTool { context, .. } => &context.connection_id,
            Operation::Replace { connection_id, .. }
            | Operation::RunNow { connection_id, .. }
            | Operation::Pending { connection_id }
            | Operation::Unsettled { connection_id }
            | Operation::Report { connection_id, .. } => connection_id,
            Operation::Connect { .. } | Operation::RegisterExecutionContext { .. } => {
                unreachable!()
            }
        };
        if self.connection.as_ref().map(|c| &c.connection_id) != Some(id) {
            return Err("schedule connection expired; reconnect".into());
        }
        if let Operation::AuthorizeTool {
            context,
            connector_id,
        } = operation
        {
            if !self.load(account)?.is_unsettled(&context.run_id) {
                self.executions.remove(&context.run_id);
                return Err("schedule run is not accepted or running".into());
            }
            let policy = self
                .executions
                .get(&context.run_id)
                .ok_or("schedule execution permissions are not registered")?;
            if !policy.connector_ids.contains(&connector_id) {
                return Err("connector is not attached to this scheduled task".into());
            }
            return Ok(Response::ToolAuthorized(ToolAuthorization {
                auto_approve: policy.auto_approve,
                read_only: policy.read_only_connector_ids.contains(&connector_id),
                connector_ids: Some(policy.connector_ids.clone()),
            }));
        }
        let store = self.load(account)?;
        match operation {
            Operation::Replace { schedules, .. } => {
                store
                    .replace_schedules(schedules, now)
                    .map_err(|e| e.to_string())?;
                store.enqueue_due(now).map_err(|e| e.to_string())?;
                Ok(Response::Done)
            }
            Operation::RunNow { schedule_id, .. } => store
                .enqueue_manual(&schedule_id, now)
                .map(Response::Queued)
                .map_err(|e| e.to_string()),
            Operation::Pending { .. } => Ok(Response::Runs(store.pending_runs())),
            Operation::Unsettled { .. } => Ok(Response::Runs(store.unsettled_runs())),
            Operation::Report { run_id, status, .. } => {
                store
                    .report_status(&run_id, status, now)
                    .map_err(|e| e.to_string())?;
                if !matches!(status, RunStatus::Accepted | RunStatus::Running) {
                    self.executions.remove(&run_id);
                }
                Ok(Response::Done)
            }
            Operation::Connect { .. }
            | Operation::RegisterExecutionContext { .. }
            | Operation::AuthorizeTool { .. } => unreachable!(),
        }
    }

    fn work_notice(&self) -> Option<WorkAvailable> {
        let connection = self.connection.as_ref()?;
        if self.loaded.as_ref()?.1.pending_runs().is_empty() {
            return None;
        }
        Some(WorkAvailable {
            connection_id: connection.connection_id.clone(),
        })
    }

    fn next_due(&self) -> Result<Option<DateTime<Utc>>, String> {
        self.loaded
            .as_ref()
            .map(|(_, store)| store.next_due().map_err(|e| e.to_string()))
            .unwrap_or(Ok(None))
    }
}

fn wait_duration(next: Option<DateTime<Utc>>, now: DateTime<Utc>, cap: Duration) -> Duration {
    next.map(|at| (at - now).to_std().unwrap_or(Duration::ZERO).min(cap))
        .unwrap_or(cap)
}

async fn run(
    mut core: CoordinatorCore,
    mut accounts: watch::Receiver<Option<AccountId>>,
    mut stop: watch::Receiver<bool>,
    mut requests: mpsc::Receiver<Request>,
    max_wait: Duration,
    mut on_check: impl FnMut(Result<usize, String>) + Send,
    mut on_work: impl FnMut(WorkAvailable) + Send,
) {
    // Start with a wall-clock check. Keep an absolute deadline between operations so
    // canceled selects cannot restart an already scheduled sleep.
    let mut deadline = Instant::now();
    loop {
        let request = tokio::select! {
            biased;
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow_and_update() { break; }
                continue;
            }
            changed = accounts.changed() => {
                if changed.is_err() { break; }
                None
            }
            _ = sleep_until(deadline) => None,
            request = requests.recv() => {
                let Some(request) = request else { break; };
                if request.reply.is_closed() { continue; }
                Some(request)
            }
        };
        if *stop.borrow() {
            break;
        }
        let (operation, expected_context, reply) = match request {
            Some(request) => (
                Some(request.operation),
                Some(request.context),
                Some(request.reply),
            ),
            None => (None, None, None),
        };
        let _ = accounts.borrow_and_update();
        let current = accounts.clone();
        let checked = tokio::task::spawn_blocking(move || {
            // Re-read at execution time, not when the bridge request entered the queue.
            let snapshot = current.borrow().clone();
            core.observe(&snapshot);
            let wake = matches!(
                &operation,
                Some(
                    Operation::Connect { .. }
                        | Operation::Replace { .. }
                        | Operation::RunNow { .. }
                )
            );
            let is_check = operation.is_none();
            let result = if expected_context
                .as_ref()
                .is_some_and(|expected| expected != &snapshot)
            {
                Err("schedule context changed; reconnect".into())
            } else {
                match operation {
                    Some(operation) => core.handle(&snapshot, operation, Utc::now()),
                    None => core
                        .check(snapshot.clone(), Utc::now())
                        .map(Response::Checked),
                }
            };
            // Recompute after successful operations, including timing replacements. Rejected
            // requests preserve the deadline so stale clients cannot delay checks or reset backoff.
            let anchor = Instant::now();
            let now = Utc::now();
            let next_deadline = if result.is_err() {
                if is_check {
                    // An overdue occurrence plus a failing disk write must not become a busy loop.
                    anchor + max_wait
                } else {
                    deadline
                }
            } else {
                anchor
                    + match core.next_due() {
                        Ok(next) => wait_duration(next, now, max_wait),
                        Err(error) => {
                            log::warn!("could not calculate scheduler wakeup; will retry: {error}");
                            max_wait
                        }
                    }
            };
            let notice = if result.is_ok()
                && (wake || matches!(&result, Ok(Response::Checked(n)) if *n > 0))
            {
                core.work_notice()
            } else {
                None
            };
            (core, snapshot, result, notice, next_deadline)
        })
        .await;
        match checked {
            Ok((returned, snapshot, result, notice, next_deadline)) => {
                core = returned;
                // A completed disk write stays in its original account's file. Never return
                // that account's data or emit a wakeup after an account change or shutdown.
                if *stop.borrow() || *accounts.borrow() != snapshot {
                    deadline = Instant::now();
                    if let Some(reply) = reply {
                        let _ = reply.send(Err("schedule context changed; reconnect".into()));
                    }
                    continue;
                }
                deadline = next_deadline;
                if let Some(notice) = notice {
                    on_work(notice);
                }
                if let Some(reply) = reply {
                    let _ = reply.send(result);
                } else {
                    on_check(result.map(|response| match response {
                        Response::Checked(n) => n,
                        _ => unreachable!(),
                    }));
                }
            }
            Err(error) => {
                log::error!("scheduler worker stopped unexpectedly: {error}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests;
