use crate::app::{BackendEvent, BackendTask};
use crate::infra::ChezmoiClient;
use crate::preview::load_file_preview;
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;
use tokio::sync::mpsc::{self, Receiver, Sender};

enum JobDone {
    Refresh,
    Mutation,
}

struct DetailJob {
    task: BackendTask,
    cancelled: Arc<AtomicBool>,
}

struct RunningDetail {
    request_id: u64,
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
struct LatestSlotState {
    queued: Option<DetailJob>,
    running: Option<RunningDetail>,
    closed: bool,
}

#[derive(Default)]
struct LatestSlot {
    state: Mutex<LatestSlotState>,
    notify: Notify,
}

impl LatestSlot {
    fn put(&self, task: BackendTask) -> Vec<u64> {
        let request_id = detail_request_id(&task).expect("detail slot received a non-detail task");
        let cancelled = Arc::new(AtomicBool::new(false));
        let job = DetailJob {
            task,
            cancelled: cancelled.clone(),
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            cancelled.store(true, Ordering::Release);
            return vec![request_id];
        }

        let mut superseded = Vec::with_capacity(2);
        if let Some(running) = state.running.take() {
            running.cancelled.store(true, Ordering::Release);
            superseded.push(running.request_id);
        }
        if let Some(previous) = state.queued.replace(job) {
            previous.cancelled.store(true, Ordering::Release);
            superseded.push(
                detail_request_id(&previous.task).expect("queued detail lost its request id"),
            );
        }
        self.notify.notify_one();
        superseded
    }

    async fn take(&self) -> Option<DetailJob> {
        loop {
            let notified = self.notify.notified();
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state.closed {
                    return None;
                }
                if let Some(job) = state.queued.take() {
                    state.running = Some(RunningDetail {
                        request_id: detail_request_id(&job.task)
                            .expect("queued detail lost its request id"),
                        cancelled: job.cancelled.clone(),
                    });
                    return Some(job);
                }
            }
            notified.await;
        }
    }

    fn finish(&self, cancelled: &Arc<AtomicBool>) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let is_current = state
            .running
            .as_ref()
            .is_some_and(|running| Arc::ptr_eq(&running.cancelled, cancelled));
        if is_current {
            state.running = None;
        }
        is_current
    }

    fn cancel_all(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        if let Some(running) = state.running.take() {
            running.cancelled.store(true, Ordering::Release);
        }
        if let Some(queued) = state.queued.take() {
            queued.cancelled.store(true, Ordering::Release);
        }
        self.notify.notify_waiters();
    }
}

struct DetailWorker {
    slot: Arc<LatestSlot>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl DetailWorker {
    fn new(client: Arc<dyn ChezmoiClient>, event_tx: Sender<BackendEvent>) -> Self {
        let slot = Arc::new(LatestSlot::default());
        let handle = tokio::spawn(detail_loop(client, slot.clone(), event_tx));
        Self {
            slot,
            handle: Some(handle),
        }
    }

    async fn shutdown(mut self) {
        self.slot.cancel_all();
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl Drop for DetailWorker {
    fn drop(&mut self) {
        self.slot.cancel_all();
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

pub async fn worker_loop(
    client: Arc<dyn ChezmoiClient>,
    mut task_rx: Receiver<BackendTask>,
    event_tx: Sender<BackendEvent>,
) {
    let detail = DetailWorker::new(client.clone(), event_tx.clone());
    let (done_tx, mut done_rx) = mpsc::channel::<JobDone>(8);
    let mut refresh_running = false;
    let mut refresh_queued = false;
    let mut mutation_running = false;
    let mut mutation_queue = VecDeque::new();

    loop {
        tokio::select! {
            task = task_rx.recv() => {
                let Some(task) = task else { break };
                tracing::debug!(task = ?task, "backend task received");
                match task {
                    BackendTask::RefreshAll => {
                        if refresh_running || mutation_running {
                            if refresh_queued {
                                send(&event_tx, BackendEvent::RefreshCoalesced).await;
                            } else {
                                refresh_queued = true;
                            }
                        } else {
                            refresh_running = true;
                            start_refresh(client.clone(), event_tx.clone(), done_tx.clone());
                        }
                    }
                    BackendTask::LoadDiff { .. }
                    | BackendTask::LoadPreview { .. }
                    | BackendTask::Inspect { .. } => {
                        for request_id in detail.slot.put(task) {
                            send(
                                &event_tx,
                                BackendEvent::DetailCoalesced { request_id },
                            )
                            .await;
                        }
                    }
                    BackendTask::PrepareApplyPlan { .. }
                    | BackendTask::ApplyReviewed { .. }
                    | BackendTask::RunAction { .. } => {
                        if mutation_running || refresh_running {
                            mutation_queue.push_back(task);
                        } else {
                            mutation_running = true;
                            start_mutation(client.clone(), event_tx.clone(), done_tx.clone(), task);
                        }
                    }
                }
            }
            Some(done) = done_rx.recv() => {
                match done {
                    JobDone::Refresh => {
                        refresh_running = false;
                        if let Some(task) = mutation_queue.pop_front() {
                            mutation_running = true;
                            start_mutation(client.clone(), event_tx.clone(), done_tx.clone(), task);
                        } else if refresh_queued {
                            refresh_queued = false;
                            refresh_running = true;
                            start_refresh(client.clone(), event_tx.clone(), done_tx.clone());
                        } else {
                            refresh_running = false;
                        }
                    }
                    JobDone::Mutation => {
                        mutation_running = false;
                        if refresh_queued {
                            refresh_queued = false;
                            refresh_running = true;
                            start_refresh(client.clone(), event_tx.clone(), done_tx.clone());
                        } else if let Some(task) = mutation_queue.pop_front() {
                            mutation_running = true;
                            start_mutation(client.clone(), event_tx.clone(), done_tx.clone(), task);
                        } else {
                            mutation_running = false;
                        }
                    }
                }
            }
        }
    }

    detail.shutdown().await;
}

fn detail_request_id(task: &BackendTask) -> Option<u64> {
    match task {
        BackendTask::LoadDiff { request_id, .. }
        | BackendTask::LoadPreview { request_id, .. }
        | BackendTask::Inspect { request_id, .. } => Some(*request_id),
        _ => None,
    }
}

fn start_refresh(
    client: Arc<dyn ChezmoiClient>,
    event_tx: Sender<BackendEvent>,
    done_tx: Sender<JobDone>,
) {
    tokio::spawn(async move {
        send(&event_tx, BackendEvent::RefreshStarted).await;

        let c1 = client.clone();
        let status = tokio::task::spawn_blocking(move || c1.status());
        let c2 = client.clone();
        let managed = tokio::task::spawn_blocking(move || c2.managed());
        let c3 = client.clone();
        let unmanaged = tokio::task::spawn_blocking(move || c3.unmanaged());
        let c4 = client.clone();
        let source = tokio::task::spawn_blocking(move || c4.source());
        let c5 = client.clone();
        let ignore = tokio::task::spawn_blocking(move || c5.ignore_patterns());

        let send_status = async {
            let result = status
                .await
                .map_err(join_error)
                .and_then(|result| result.map_err(error_text));
            send(&event_tx, BackendEvent::StatusLoaded(result)).await;
        };
        let send_managed = async {
            let result = managed
                .await
                .map_err(join_error)
                .and_then(|result| result.map_err(error_text));
            send(&event_tx, BackendEvent::ManagedLoaded(result)).await;
        };
        let send_unmanaged = async {
            let result = unmanaged
                .await
                .map_err(join_error)
                .and_then(|result| result.map_err(error_text));
            send(&event_tx, BackendEvent::UnmanagedLoaded(result)).await;
        };
        let send_source = async {
            let result = source
                .await
                .map_err(join_error)
                .and_then(|result| result.map_err(error_text));
            send(&event_tx, BackendEvent::SourceLoaded(result)).await;
        };
        let send_ignore = async {
            let result = ignore
                .await
                .map_err(join_error)
                .and_then(|result| result.map_err(error_text));
            send(&event_tx, BackendEvent::IgnoreLoaded(result)).await;
        };

        tokio::join!(
            send_status,
            send_managed,
            send_unmanaged,
            send_source,
            send_ignore
        );
        send(&event_tx, BackendEvent::RefreshFinished).await;
        let _ = done_tx.send(JobDone::Refresh).await;
    });
}

fn start_mutation(
    client: Arc<dyn ChezmoiClient>,
    event_tx: Sender<BackendEvent>,
    done_tx: Sender<JobDone>,
    task: BackendTask,
) {
    tokio::spawn(async move {
        match task {
            BackendTask::PrepareApplyPlan {
                request_id,
                request,
                destination,
            } => {
                let c = client.clone();
                let targets = request.targets.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let status = c.status()?;
                    let diff = c.diff(&targets)?;
                    let plan = crate::app::build_apply_plan(&status, &destination, &targets);
                    let snapshot = crate::app::ApplyPlanSnapshot {
                        targets,
                        plan,
                        diff_fingerprint: fingerprint_text(&diff.text),
                    };
                    Ok::<_, anyhow::Error>((status, snapshot))
                })
                .await;

                match result {
                    Ok(Ok((status, snapshot))) => {
                        send(
                            &event_tx,
                            BackendEvent::ApplyPlanPrepared {
                                request_id,
                                request,
                                status,
                                snapshot,
                            },
                        )
                        .await;
                    }
                    other => {
                        send(
                            &event_tx,
                            BackendEvent::Error {
                                context: "apply-plan".to_string(),
                                message: format!(
                                    "apply plan refresh failed: {:?}",
                                    flatten_error(other)
                                ),
                                mutation: false,
                            },
                        )
                        .await;
                    }
                }
            }
            BackendTask::ApplyReviewed {
                request_id,
                request,
                destination,
                expected_snapshot,
            } => {
                let c = client.clone();
                let reviewed_request = request.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let status = c.status()?;
                    let diff = c.diff(&reviewed_request.targets)?;
                    let snapshot = crate::app::ApplyPlanSnapshot {
                        targets: reviewed_request.targets.clone(),
                        plan: crate::app::build_apply_plan(
                            &status,
                            &destination,
                            &reviewed_request.targets,
                        ),
                        diff_fingerprint: fingerprint_text(&diff.text),
                    };
                    if snapshot != expected_snapshot {
                        Ok::<_, anyhow::Error>((status, snapshot, None))
                    } else {
                        let finished = c.run(&reviewed_request)?;
                        Ok((status, snapshot, Some(finished)))
                    }
                })
                .await;

                match result {
                    Ok(Ok((_status, _snapshot, Some(finished)))) => {
                        send(
                            &event_tx,
                            BackendEvent::ActionFinished {
                                request,
                                result: finished,
                            },
                        )
                        .await;
                    }
                    Ok(Ok((status, snapshot, None))) => {
                        send(
                            &event_tx,
                            BackendEvent::ApplyPlanChanged {
                                request_id,
                                request,
                                status,
                                snapshot,
                            },
                        )
                        .await;
                    }
                    other => {
                        send(
                            &event_tx,
                            BackendEvent::Error {
                                context: "apply-reviewed".to_string(),
                                message: format!(
                                    "apply validation failed: {:?}",
                                    flatten_error(other)
                                ),
                                mutation: true,
                            },
                        )
                        .await;
                    }
                }
            }
            BackendTask::RunAction { request } => {
                let c = client.clone();
                let req = request.clone();
                let result = tokio::task::spawn_blocking(move || c.run(&req)).await;
                match result {
                    Ok(Ok(result)) => {
                        send(&event_tx, BackendEvent::ActionFinished { request, result }).await;
                    }
                    other => {
                        send(
                            &event_tx,
                            BackendEvent::Error {
                                context: "action".to_string(),
                                message: format!("action failed: {:?}", flatten_error(other)),
                                mutation: !request.action.is_read_only(),
                            },
                        )
                        .await;
                    }
                }
            }
            _ => unreachable!("mutation worker received a read-only task"),
        }
        let _ = done_tx.send(JobDone::Mutation).await;
    });
}

async fn detail_loop(
    client: Arc<dyn ChezmoiClient>,
    slot: Arc<LatestSlot>,
    event_tx: Sender<BackendEvent>,
) {
    while let Some(job) = slot.take().await {
        let DetailJob { task, cancelled } = job;
        match task {
            BackendTask::LoadDiff { request_id, target } => {
                let c = client.clone();
                let worker_target = target.clone();
                let targets = worker_target.into_iter().collect::<Vec<_>>();
                let detail_cancelled = cancelled.clone();
                let result = tokio::task::spawn_blocking(move || {
                    c.diff_cancellable(&targets, &detail_cancelled)
                })
                .await;
                if !slot.finish(&cancelled) {
                    continue;
                }
                match result {
                    Ok(Ok(diff)) => {
                        send(
                            &event_tx,
                            BackendEvent::DiffLoaded {
                                request_id,
                                target,
                                diff,
                            },
                        )
                        .await;
                    }
                    other => {
                        send(
                            &event_tx,
                            BackendEvent::DetailError {
                                request_id,
                                context: "diff".to_string(),
                                message: format!("diff failed: {:?}", flatten_error(other)),
                            },
                        )
                        .await;
                    }
                }
            }
            BackendTask::LoadPreview {
                request_id,
                target,
                absolute,
                origin,
            } => {
                let result =
                    tokio::task::spawn_blocking(move || load_file_preview(&absolute)).await;
                if !slot.finish(&cancelled) {
                    continue;
                }
                match result {
                    Ok(Ok(content)) => {
                        send(
                            &event_tx,
                            BackendEvent::PreviewLoaded {
                                request_id,
                                target,
                                origin,
                                content,
                            },
                        )
                        .await;
                    }
                    other => {
                        send(
                            &event_tx,
                            BackendEvent::DetailError {
                                request_id,
                                context: "preview".to_string(),
                                message: format!("preview failed: {:?}", flatten_error(other)),
                            },
                        )
                        .await;
                    }
                }
            }
            BackendTask::Inspect {
                request_id,
                request,
            } => {
                let c = client.clone();
                let inspection_request = request.clone();
                let detail_cancelled = cancelled.clone();
                let result = tokio::task::spawn_blocking(move || {
                    c.run_cancellable(&inspection_request, &detail_cancelled)
                })
                .await;
                if !slot.finish(&cancelled) {
                    continue;
                }
                match result {
                    Ok(Ok(result)) => {
                        send(
                            &event_tx,
                            BackendEvent::InspectionFinished {
                                request_id,
                                request,
                                result,
                            },
                        )
                        .await;
                    }
                    other => {
                        send(
                            &event_tx,
                            BackendEvent::DetailError {
                                request_id,
                                context: request.action.label().to_string(),
                                message: format!("inspection failed: {:?}", flatten_error(other)),
                            },
                        )
                        .await;
                    }
                }
            }
            _ => unreachable!("detail worker received a non-detail task"),
        }
    }
}

async fn send(event_tx: &Sender<BackendEvent>, event: BackendEvent) {
    let _ = event_tx.send(event).await;
}

fn join_error(error: tokio::task::JoinError) -> String {
    format!("join error: {error}")
}

fn error_text(error: anyhow::Error) -> String {
    format!("{error:#}")
}

fn flatten_error<T>(res: std::result::Result<anyhow::Result<T>, tokio::task::JoinError>) -> String {
    match res {
        Ok(Ok(_)) => "ok".to_string(),
        Ok(Err(err)) => format!("{err:#}"),
        Err(err) => format!("join error: {err}"),
    }
}

fn fingerprint_text(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ActionRequest, CommandResult, DiffText, StatusEntry};
    use anyhow::Result;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    #[derive(Default)]
    struct SlowMock {
        diff_calls: AtomicUsize,
    }

    #[derive(Default)]
    struct SequencingMock {
        mutation_calls: AtomicUsize,
        refresh_active: AtomicBool,
        mutation_active: AtomicBool,
        overlap: AtomicBool,
    }

    #[derive(Default)]
    struct ApplyRaceMock {
        diff_calls: AtomicUsize,
        mutation_calls: AtomicUsize,
        diff_changes: bool,
        fail_run: bool,
    }

    #[derive(Default)]
    struct CancellableDetailMock {
        plain_diff_calls: AtomicUsize,
        cancellable_diff_calls: AtomicUsize,
        plain_run_calls: AtomicUsize,
        cancellable_run_calls: AtomicUsize,
        old_diff_started: AtomicBool,
        old_diff_cancelled: AtomicBool,
        inspection_started: AtomicBool,
        inspection_cancelled: AtomicBool,
    }

    impl CancellableDetailMock {
        fn wait_for_cancel(cancelled: &AtomicBool) -> Result<()> {
            for _ in 0..200 {
                if cancelled.load(Ordering::Acquire) {
                    return Err(anyhow::anyhow!("detail cancelled"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(anyhow::anyhow!("mock cancellation timeout"))
        }

        fn ok_command() -> CommandResult {
            CommandResult {
                exit_code: 0,
                stdout: "ok".to_string(),
                stderr: String::new(),
                duration_ms: 1,
                timed_out: false,
                output_limited: false,
                stdout_truncated: false,
                stderr_truncated: false,
            }
        }
    }

    impl ChezmoiClient for CancellableDetailMock {
        fn status(&self) -> Result<Vec<StatusEntry>> {
            Ok(Vec::new())
        }

        fn managed(&self) -> Result<Vec<PathBuf>> {
            Ok(Vec::new())
        }

        fn unmanaged(&self) -> Result<Vec<PathBuf>> {
            Ok(Vec::new())
        }

        fn ignore_patterns(&self) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        fn source(&self) -> Result<(PathBuf, Vec<PathBuf>)> {
            Ok((PathBuf::from("/source"), Vec::new()))
        }

        fn diff(&self, targets: &[PathBuf]) -> Result<DiffText> {
            self.plain_diff_calls.fetch_add(1, Ordering::SeqCst);
            Ok(DiffText {
                text: format!("plain:{}", targets[0].display()),
            })
        }

        fn diff_cancellable(
            &self,
            targets: &[PathBuf],
            cancelled: &AtomicBool,
        ) -> Result<DiffText> {
            self.cancellable_diff_calls.fetch_add(1, Ordering::SeqCst);
            if targets == [PathBuf::from("old")] {
                self.old_diff_started.store(true, Ordering::Release);
                let error = Self::wait_for_cancel(cancelled);
                if cancelled.load(Ordering::Acquire) {
                    self.old_diff_cancelled.store(true, Ordering::Release);
                }
                error?;
            }
            Ok(DiffText {
                text: format!("cancellable:{}", targets[0].display()),
            })
        }

        fn run(&self, _request: &ActionRequest) -> Result<CommandResult> {
            self.plain_run_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Self::ok_command())
        }

        fn run_cancellable(
            &self,
            request: &ActionRequest,
            cancelled: &AtomicBool,
        ) -> Result<CommandResult> {
            self.cancellable_run_calls.fetch_add(1, Ordering::SeqCst);
            if request.targets == [PathBuf::from("slow-inspect")] {
                self.inspection_started.store(true, Ordering::Release);
                let error = Self::wait_for_cancel(cancelled);
                if cancelled.load(Ordering::Acquire) {
                    self.inspection_cancelled.store(true, Ordering::Release);
                }
                error?;
            }
            Ok(Self::ok_command())
        }
    }

    impl ChezmoiClient for ApplyRaceMock {
        fn status(&self) -> Result<Vec<StatusEntry>> {
            Ok(vec![StatusEntry {
                path: PathBuf::from(".zshrc"),
                actual_vs_state: crate::domain::ChangeKind::None,
                actual_vs_target: crate::domain::ChangeKind::Modified,
            }])
        }

        fn managed(&self) -> Result<Vec<PathBuf>> {
            Ok(Vec::new())
        }

        fn unmanaged(&self) -> Result<Vec<PathBuf>> {
            Ok(Vec::new())
        }

        fn ignore_patterns(&self) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        fn source(&self) -> Result<(PathBuf, Vec<PathBuf>)> {
            Ok((PathBuf::from("/source"), Vec::new()))
        }

        fn diff(&self, _targets: &[PathBuf]) -> Result<DiffText> {
            let call = self.diff_calls.fetch_add(1, Ordering::SeqCst);
            Ok(DiffText {
                text: if self.diff_changes && call > 0 {
                    "changed".to_string()
                } else {
                    "reviewed".to_string()
                },
            })
        }

        fn run(&self, _request: &ActionRequest) -> Result<CommandResult> {
            if self.fail_run {
                return Err(anyhow::anyhow!("inspection command failed"));
            }
            self.mutation_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CommandResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: 1,
                timed_out: false,
                output_limited: false,
                stdout_truncated: false,
                stderr_truncated: false,
            })
        }
    }

    impl ChezmoiClient for SequencingMock {
        fn status(&self) -> Result<Vec<StatusEntry>> {
            if self.mutation_active.load(Ordering::SeqCst) {
                self.overlap.store(true, Ordering::SeqCst);
            }
            self.refresh_active.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(40));
            self.refresh_active.store(false, Ordering::SeqCst);
            Ok(Vec::new())
        }

        fn managed(&self) -> Result<Vec<PathBuf>> {
            Ok(Vec::new())
        }

        fn unmanaged(&self) -> Result<Vec<PathBuf>> {
            Ok(Vec::new())
        }

        fn ignore_patterns(&self) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        fn source(&self) -> Result<(PathBuf, Vec<PathBuf>)> {
            Ok((PathBuf::from("/source"), Vec::new()))
        }

        fn diff(&self, _targets: &[PathBuf]) -> Result<DiffText> {
            Ok(DiffText {
                text: String::new(),
            })
        }

        fn run(&self, request: &ActionRequest) -> Result<CommandResult> {
            self.mutation_calls.fetch_add(1, Ordering::SeqCst);
            if self.refresh_active.load(Ordering::SeqCst) {
                self.overlap.store(true, Ordering::SeqCst);
            }
            self.mutation_active.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(40));
            self.mutation_active.store(false, Ordering::SeqCst);
            Ok(CommandResult {
                exit_code: 0,
                stdout: request
                    .targets
                    .first()
                    .map_or_else(String::new, |target| target.display().to_string()),
                stderr: String::new(),
                duration_ms: 40,
                timed_out: false,
                output_limited: false,
                stdout_truncated: false,
                stderr_truncated: false,
            })
        }
    }

    impl ChezmoiClient for SlowMock {
        fn status(&self) -> Result<Vec<StatusEntry>> {
            Ok(Vec::new())
        }

        fn managed(&self) -> Result<Vec<PathBuf>> {
            Ok(vec![PathBuf::from(".managed")])
        }

        fn unmanaged(&self) -> Result<Vec<PathBuf>> {
            Err(anyhow::anyhow!("unmanaged failed"))
        }

        fn ignore_patterns(&self) -> Result<Vec<String>> {
            Err(anyhow::anyhow!("ignore failed"))
        }

        fn source(&self) -> Result<(PathBuf, Vec<PathBuf>)> {
            std::thread::sleep(Duration::from_millis(150));
            Ok((PathBuf::from("/source"), vec![PathBuf::from("dot_zshrc")]))
        }

        fn diff(&self, targets: &[PathBuf]) -> Result<DiffText> {
            let call = self.diff_calls.fetch_add(1, Ordering::SeqCst);
            if targets == [PathBuf::from("old")] {
                std::thread::sleep(Duration::from_millis(120));
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(DiffText {
                text: format!(
                    "{}:{call}",
                    targets
                        .first()
                        .map_or_else(|| "all".to_string(), |target| target.display().to_string())
                ),
            })
        }

        fn run(&self, _request: &ActionRequest) -> Result<CommandResult> {
            unreachable!()
        }
    }

    async fn wait_until(flag: &AtomicBool) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !flag.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timed out waiting for mock signal");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refresh_parts_arrive_independently_and_failures_are_per_part() {
        let client = Arc::new(SlowMock::default());
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let worker = tokio::spawn(worker_loop(client, task_rx, event_tx));
        task_tx.send(BackendTask::RefreshAll).await.unwrap();

        assert!(matches!(
            event_rx.recv().await,
            Some(BackendEvent::RefreshStarted)
        ));
        let mut order = Vec::new();
        let mut unmanaged_error = false;
        let mut ignore_error = false;
        for _ in 0..6 {
            match event_rx.recv().await.unwrap() {
                BackendEvent::StatusLoaded(result) => {
                    result.unwrap();
                    order.push("status");
                }
                BackendEvent::ManagedLoaded(result) => {
                    result.unwrap();
                    order.push("managed");
                }
                BackendEvent::UnmanagedLoaded(result) => {
                    unmanaged_error = result.is_err();
                    order.push("unmanaged");
                }
                BackendEvent::SourceLoaded(result) => {
                    result.unwrap();
                    order.push("source");
                }
                BackendEvent::IgnoreLoaded(result) => {
                    ignore_error = result.is_err();
                    order.push("ignore");
                }
                BackendEvent::RefreshFinished => break,
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert!(unmanaged_error && ignore_error);
        assert!(
            order.iter().position(|part| *part == "status")
                < order.iter().position(|part| *part == "source")
        );
        drop(task_tx);
        worker.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn detail_work_is_not_blocked_by_slow_refresh() {
        let client = Arc::new(SlowMock::default());
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let worker = tokio::spawn(worker_loop(client, task_rx, event_tx));

        task_tx.send(BackendTask::RefreshAll).await.unwrap();
        task_tx
            .send(BackendTask::LoadDiff {
                request_id: 1,
                target: Some(PathBuf::from("current")),
            })
            .await
            .unwrap();

        loop {
            match event_rx.recv().await.unwrap() {
                BackendEvent::DiffLoaded { request_id, .. } => {
                    assert_eq!(request_id, 1);
                    break;
                }
                BackendEvent::RefreshFinished => {
                    panic!("slow refresh completed before detail result")
                }
                _ => {}
            }
        }

        drop(task_tx);
        worker.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn latest_detail_request_wins_and_obsolete_queued_request_is_reported() {
        let client = Arc::new(SlowMock::default());
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let worker = tokio::spawn(worker_loop(client, task_rx, event_tx));
        task_tx
            .send(BackendTask::LoadDiff {
                request_id: 1,
                target: Some(PathBuf::from("old")),
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        task_tx
            .send(BackendTask::LoadDiff {
                request_id: 2,
                target: Some(PathBuf::from("middle")),
            })
            .await
            .unwrap();
        task_tx
            .send(BackendTask::LoadDiff {
                request_id: 3,
                target: Some(PathBuf::from("new")),
            })
            .await
            .unwrap();

        let mut coalesced = Vec::new();
        let mut loaded = Vec::new();
        while loaded.is_empty() {
            match event_rx.recv().await.unwrap() {
                BackendEvent::DetailCoalesced { request_id } => coalesced.push(request_id),
                BackendEvent::DiffLoaded {
                    request_id, target, ..
                } => loaded.push((request_id, target)),
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert_eq!(coalesced, vec![1, 2]);
        assert_eq!(loaded, vec![(3, Some(PathBuf::from("new")))]);
        drop(task_tx);
        worker.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn running_detail_is_cancelled_without_stale_error() {
        let client = Arc::new(CancellableDetailMock::default());
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let worker = tokio::spawn(worker_loop(client.clone(), task_rx, event_tx));

        task_tx
            .send(BackendTask::LoadDiff {
                request_id: 10,
                target: Some(PathBuf::from("old")),
            })
            .await
            .unwrap();
        wait_until(&client.old_diff_started).await;
        task_tx
            .send(BackendTask::LoadDiff {
                request_id: 11,
                target: Some(PathBuf::from("new")),
            })
            .await
            .unwrap();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
                .await
                .expect("timed out"),
            Some(BackendEvent::DetailCoalesced { request_id: 10 })
        ));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
                .await
                .expect("timed out"),
            Some(BackendEvent::DiffLoaded {
                request_id: 11,
                diff,
                ..
            }) if diff.text == "cancellable:new"
        ));
        assert!(client.old_diff_cancelled.load(Ordering::Acquire));
        assert_eq!(client.cancellable_diff_calls.load(Ordering::SeqCst), 2);
        assert_eq!(client.plain_diff_calls.load(Ordering::SeqCst), 0);

        drop(task_tx);
        worker.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn inspection_uses_latest_detail_lane_and_cancels_running_command() {
        let client = Arc::new(CancellableDetailMock::default());
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let worker = tokio::spawn(worker_loop(client.clone(), task_rx, event_tx));

        task_tx
            .send(BackendTask::Inspect {
                request_id: 20,
                request: ActionRequest {
                    action: crate::domain::Action::Cat,
                    targets: vec![PathBuf::from("slow-inspect")],
                    chattr_attrs: None,
                },
            })
            .await
            .unwrap();
        wait_until(&client.inspection_started).await;
        task_tx
            .send(BackendTask::LoadDiff {
                request_id: 21,
                target: Some(PathBuf::from("new")),
            })
            .await
            .unwrap();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
                .await
                .expect("timed out"),
            Some(BackendEvent::DetailCoalesced { request_id: 20 })
        ));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
                .await
                .expect("timed out"),
            Some(BackendEvent::DiffLoaded { request_id: 21, .. })
        ));
        assert!(client.inspection_cancelled.load(Ordering::Acquire));
        assert_eq!(client.cancellable_run_calls.load(Ordering::SeqCst), 1);
        assert_eq!(client.plain_run_calls.load(Ordering::SeqCst), 0);

        drop(task_tx);
        worker.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn running_detail_does_not_redirect_mutation_run() {
        let client = Arc::new(CancellableDetailMock::default());
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let worker = tokio::spawn(worker_loop(client.clone(), task_rx, event_tx));

        task_tx
            .send(BackendTask::LoadDiff {
                request_id: 25,
                target: Some(PathBuf::from("old")),
            })
            .await
            .unwrap();
        wait_until(&client.old_diff_started).await;
        task_tx
            .send(BackendTask::RunAction {
                request: ActionRequest {
                    action: crate::domain::Action::Forget,
                    targets: vec![PathBuf::from("mutation")],
                    chattr_attrs: None,
                },
            })
            .await
            .unwrap();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
                .await
                .expect("timed out"),
            Some(BackendEvent::ActionFinished { .. })
        ));
        assert_eq!(client.plain_run_calls.load(Ordering::SeqCst), 1);
        assert_eq!(client.cancellable_run_calls.load(Ordering::SeqCst), 0);
        assert!(!client.old_diff_cancelled.load(Ordering::Acquire));

        task_tx
            .send(BackendTask::LoadDiff {
                request_id: 26,
                target: Some(PathBuf::from("new")),
            })
            .await
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
                .await
                .expect("timed out"),
            Some(BackendEvent::DetailCoalesced { request_id: 25 })
        ));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
                .await
                .expect("timed out"),
            Some(BackendEvent::DiffLoaded { request_id: 26, .. })
        ));

        drop(task_tx);
        worker.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn worker_shutdown_cancels_running_detail_token() {
        let client = Arc::new(CancellableDetailMock::default());
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, _event_rx) = mpsc::channel(8);
        let worker = tokio::spawn(worker_loop(client.clone(), task_rx, event_tx));

        task_tx
            .send(BackendTask::LoadDiff {
                request_id: 30,
                target: Some(PathBuf::from("old")),
            })
            .await
            .unwrap();
        wait_until(&client.old_diff_started).await;
        worker.abort();
        let _ = worker.await;

        tokio::time::timeout(Duration::from_secs(1), async {
            while !client.old_diff_cancelled.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("worker shutdown did not cancel the detail token");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn refresh_then_queued_refresh_then_mutation_stays_serialized() {
        let client = Arc::new(SequencingMock::default());
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let worker = tokio::spawn(worker_loop(client.clone(), task_rx, event_tx));

        task_tx.send(BackendTask::RefreshAll).await.unwrap();
        assert!(matches!(
            event_rx.recv().await,
            Some(BackendEvent::RefreshStarted)
        ));
        task_tx.send(BackendTask::RefreshAll).await.unwrap();
        task_tx
            .send(BackendTask::RunAction {
                request: ActionRequest {
                    action: crate::domain::Action::Forget,
                    targets: vec![PathBuf::from("a")],
                    chattr_attrs: None,
                },
            })
            .await
            .unwrap();

        let mut order: Vec<String> = vec!["refresh".to_string()];
        while order
            .iter()
            .filter(|event| event.as_str() == "refresh")
            .count()
            < 2
            || !order.iter().any(|event| event == "mutation:a")
        {
            let event = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for ordered events: {order:?}"))
                .unwrap();
            match event {
                BackendEvent::RefreshStarted => order.push("refresh".to_string()),
                BackendEvent::ActionFinished { request, .. } => {
                    order.push(format!("mutation:{}", request.targets[0].display()))
                }
                _ => {}
            }
        }

        assert_eq!(order, vec!["refresh", "mutation:a", "refresh"]);
        assert!(!client.overlap.load(Ordering::SeqCst));
        drop(task_tx);
        worker.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mutation_then_queued_refresh_then_second_mutation_runs_in_order() {
        let client = Arc::new(SequencingMock::default());
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let worker = tokio::spawn(worker_loop(client.clone(), task_rx, event_tx));

        task_tx
            .send(BackendTask::RunAction {
                request: ActionRequest {
                    action: crate::domain::Action::Forget,
                    targets: vec![PathBuf::from("a")],
                    chattr_attrs: None,
                },
            })
            .await
            .unwrap();
        while client.mutation_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        task_tx.send(BackendTask::RefreshAll).await.unwrap();
        task_tx
            .send(BackendTask::RunAction {
                request: ActionRequest {
                    action: crate::domain::Action::Forget,
                    targets: vec![PathBuf::from("b")],
                    chattr_attrs: None,
                },
            })
            .await
            .unwrap();

        let mut order: Vec<String> = Vec::new();
        while order
            .iter()
            .filter(|event| event.starts_with("mutation:"))
            .count()
            < 2
            || !order.iter().any(|event| event == "refresh")
        {
            let event = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for ordered events: {order:?}"))
                .unwrap();
            match event {
                BackendEvent::RefreshStarted => order.push("refresh".to_string()),
                BackendEvent::ActionFinished { request, .. } => {
                    order.push(format!("mutation:{}", request.targets[0].display()))
                }
                _ => {}
            }
        }

        assert_eq!(order, vec!["mutation:a", "refresh", "mutation:b"]);
        assert!(!client.overlap.load(Ordering::SeqCst));
        drop(task_tx);
        worker.await.unwrap();
    }

    #[test]
    fn flatten_error_formats_all_cases() {
        let ok = flatten_error::<()>(Ok(Ok(())));
        assert_eq!(ok, "ok");

        let err = flatten_error::<()>(Ok(Err(anyhow::anyhow!("boom"))));
        assert!(err.contains("boom"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn apply_reviewed_rechecks_and_never_runs_after_a_changed_snapshot() {
        let client = Arc::new(ApplyRaceMock {
            diff_changes: true,
            ..ApplyRaceMock::default()
        });
        // Model the diff already reviewed before the confirmation opened; the
        // recheck inside ApplyReviewed must be the next call and observe a change.
        client.diff_calls.store(1, Ordering::SeqCst);
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let worker = tokio::spawn(worker_loop(client.clone(), task_rx, event_tx));
        let targets = vec![PathBuf::from("/tmp/.zshrc")];
        let expected_snapshot = crate::app::ApplyPlanSnapshot {
            targets: targets.clone(),
            plan: crate::app::ApplyPlan {
                modified: vec![PathBuf::from(".zshrc")],
                ..crate::app::ApplyPlan::default()
            },
            diff_fingerprint: fingerprint_text("reviewed"),
        };

        task_tx
            .send(BackendTask::ApplyReviewed {
                request_id: 1,
                request: ActionRequest {
                    action: crate::domain::Action::Apply,
                    targets,
                    chattr_attrs: None,
                },
                destination: PathBuf::from("/tmp"),
                expected_snapshot,
            })
            .await
            .unwrap();

        match tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
            .await
            .expect("timed out")
            .expect("event")
        {
            BackendEvent::ApplyPlanChanged { snapshot, .. } => {
                assert_eq!(snapshot.diff_fingerprint, fingerprint_text("changed"));
            }
            other => panic!("expected changed plan, got {other:?}"),
        }
        assert_eq!(client.mutation_calls.load(Ordering::SeqCst), 0);
        drop(task_tx);
        worker.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn apply_reviewed_runs_once_when_the_snapshot_is_unchanged() {
        let client = Arc::new(ApplyRaceMock::default());
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let worker = tokio::spawn(worker_loop(client.clone(), task_rx, event_tx));
        let targets = vec![PathBuf::from("/tmp/.zshrc")];
        let expected_snapshot = crate::app::ApplyPlanSnapshot {
            targets: targets.clone(),
            plan: crate::app::ApplyPlan {
                modified: vec![PathBuf::from(".zshrc")],
                ..crate::app::ApplyPlan::default()
            },
            diff_fingerprint: fingerprint_text("reviewed"),
        };
        let request = ActionRequest {
            action: crate::domain::Action::Apply,
            targets,
            chattr_attrs: None,
        };

        task_tx
            .send(BackendTask::ApplyReviewed {
                request_id: 1,
                request: request.clone(),
                destination: PathBuf::from("/tmp"),
                expected_snapshot,
            })
            .await
            .unwrap();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
                .await
                .expect("timed out"),
            Some(BackendEvent::ActionFinished {
                request: finished,
                ..
            }) if finished == request
        ));
        assert_eq!(client.mutation_calls.load(Ordering::SeqCst), 1);
        drop(task_tx);
        worker.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inspection_failure_keeps_its_request_identity() {
        let client = Arc::new(ApplyRaceMock {
            fail_run: true,
            ..ApplyRaceMock::default()
        });
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let worker = tokio::spawn(worker_loop(client, task_rx, event_tx));

        task_tx
            .send(BackendTask::Inspect {
                request_id: 42,
                request: ActionRequest {
                    action: crate::domain::Action::Cat,
                    targets: vec![PathBuf::from("/tmp/.zshrc")],
                    chattr_attrs: None,
                },
            })
            .await
            .unwrap();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
                .await
                .expect("timed out"),
            Some(BackendEvent::DetailError {
                request_id: 42,
                context,
                message,
            }) if context == "cat" && message.contains("inspection command failed")
        ));
        drop(task_tx);
        worker.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inspection_success_keeps_its_request_identity() {
        let client = Arc::new(ApplyRaceMock::default());
        let (task_tx, task_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let worker = tokio::spawn(worker_loop(client.clone(), task_rx, event_tx));
        let request = ActionRequest {
            action: crate::domain::Action::Cat,
            targets: vec![PathBuf::from("/tmp/.zshrc")],
            chattr_attrs: None,
        };

        task_tx
            .send(BackendTask::Inspect {
                request_id: 43,
                request: request.clone(),
            })
            .await
            .unwrap();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
                .await
                .expect("timed out"),
            Some(BackendEvent::InspectionFinished {
                request_id: 43,
                request: finished,
                ..
            }) if finished == request
        ));
        assert_eq!(client.mutation_calls.load(Ordering::SeqCst), 1);
        drop(task_tx);
        worker.await.unwrap();
    }
}
