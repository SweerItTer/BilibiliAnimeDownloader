//! One bounded command reducer and supervised job set; shells own the runtime.
use crate::{
    api::Api,
    auth::{AuthAction, AuthManager, AuthSnapshot},
    cookie::CredentialJar,
    domain::*,
    state::{self, Completion, JobRecord, Journal, LocalMerge, ResourceRecord},
};
use futures_util::FutureExt;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, RwLock},
};
use tokio::{
    runtime::Handle,
    sync::{broadcast, mpsc, oneshot, watch, Mutex, Semaphore},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

type Reply<T> = oneshot::Sender<Result<T>>;
enum Command {
    Enqueue(Box<StreamPlan>, Reply<JobId>),
    Control(JobId, JobAction, Reply<()>),
    Merge(MergeRequest, Reply<JobId>),
    Clean(CleanupRequest, Reply<CleanupReport>),
    Shutdown(StopMode, Reply<ShutdownReport>),
    Update(JobId, Update, Reply<()>),
    Progress(JobId, u64),
}
enum Update {
    Phase(JobPhase),
    Resources(Vec<ResourceRecord>),
    Components(Vec<ComponentResult>),
    Commit(Completion),
    Promoted,
    Skipped,
}
struct CoreInner {
    config: CoreConfig,
    api: Arc<Api>,
    auth: Arc<Mutex<AuthManager>>,
    commands: mpsc::Sender<Command>,
    snapshots: watch::Receiver<Arc<QueueSnapshot>>,
    events: broadcast::Sender<CoreEvent>,
    supervisor: Mutex<Option<JoinHandle<()>>>,
    stopped: Mutex<Option<ShutdownReport>>,
    abort: CancellationToken,
    closing: std::sync::atomic::AtomicBool,
}
/// Graceful shutdown must be awaited while the supplied Tokio runtime is alive.
/// Drop is a non-graceful cancellation request; durable recovery remains available.
#[derive(Clone)]
pub struct Core {
    inner: Arc<CoreInner>,
}

impl Core {
    pub fn new(config: CoreConfig, runtime: Handle) -> Result<Self> {
        if config.max_jobs == 0
            || config.max_jobs > 128
            || config.max_connections == 0
            || config.max_connections > 256
            || config.retry.attempts == 0
            || !config.output_dir.is_absolute()
        {
            return Err(CoreError::new(
                ErrorCode::InvalidInput,
                "invalid core limits or output directory",
            ));
        }
        let journal = Arc::new(Journal::open(&config.state_dir, config.state_mode)?);
        let (mut revision, mut records) = journal.load()?;
        if config.state_mode == StateMode::ReadWrite {
            let mut recovered = HashSet::new();
            for record in &mut records {
                // Large media verification belongs to tracked job workers,
                // never the synchronous constructor or command reducer.
                if record.snapshot.phase.is_active() {
                    record.snapshot.phase = JobPhase::Paused;
                    recovered.insert(record.snapshot.id);
                }
            }
            if !recovered.is_empty() || records.is_empty() {
                revision = revision.checked_add(1).ok_or_else(|| {
                    CoreError::new(ErrorCode::Corruption, "queue revision overflow")
                })?;
                for record in &mut records {
                    if recovered.contains(&record.snapshot.id) {
                        record.snapshot.revision = revision;
                    }
                }
                journal.save(revision, &mut records, Some(&recovered))?;
            }
        }
        // No automatic redirect can replay account headers to another origin.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(config.timeout)
            .user_agent(crate::api::USER_AGENT)
            .build()
            .map_err(|_| CoreError::new(ErrorCode::Transport, "create API client"))?;
        let media_client = media_client(config.timeout)?;
        let credentials = Arc::new(RwLock::new(CredentialJar::default()));
        let connections = Arc::new(Semaphore::new(config.max_connections));
        let api = Arc::new(Api::new(
            client.clone(),
            credentials.clone(),
            connections.clone(),
        ));
        let auth = Arc::new(Mutex::new(AuthManager::new(
            client.clone(),
            credentials.clone(),
            config.credential_store.clone(),
            connections.clone(),
        )));
        let (commands, receiver) = mpsc::channel(128);
        let cache: HashMap<JobId, Arc<JobSnapshot>> = records
            .iter()
            .map(|r| (r.snapshot.id, Arc::new(r.snapshot.clone())))
            .collect();
        let (snapshots_tx, snapshots) = watch::channel(Arc::new(QueueSnapshot {
            schema_version: 1,
            revision,
            jobs: records
                .iter()
                .map(|r| cache[&r.snapshot.id].clone())
                .collect(),
        }));
        let (events, _) = broadcast::channel(256);
        let abort = CancellationToken::new();
        let runner = Reducer {
            config: config.clone(),
            journal,
            records,
            revision,
            cache,
            dirty: HashSet::new(),
            failure: None,
            auth: auth.clone(),
            snapshots: snapshots_tx,
            events: events.clone(),
            receiver,
            sender: commands.clone(),
            api: api.clone(),
            client: media_client,
            sidecar_client: client.clone(),
            credentials: credentials.clone(),
            connections,
            merges: Arc::new(Semaphore::new(1)),
            queue: VecDeque::new(),
            workers: JoinSet::new(),
            active: HashMap::new(),
            deferred: HashMap::new(),
            stopping: None,
            abort: abort.clone(),
        };
        let supervisor = runtime.spawn(runner.run());
        Ok(Self {
            inner: Arc::new(CoreInner {
                config,
                api,
                auth,
                commands,
                snapshots,
                events,
                supervisor: Mutex::new(Some(supervisor)),
                stopped: Mutex::new(None),
                abort,
                closing: std::sync::atomic::AtomicBool::new(false),
            }),
        })
    }
    pub async fn resolve(&self, input: ContentInput) -> Result<MediaItem> {
        self.ensure_open()?;
        self.inner.api.resolve(input).await
    }
    pub async fn search(&self, query: SearchQuery) -> Result<CandidatePage> {
        self.ensure_open()?;
        self.inner.api.search(query).await
    }
    pub async fn cover(&self, request: CoverRequest) -> Result<CoverBytes> {
        self.ensure_open()?;
        self.inner.api.cover(request).await
    }
    pub async fn quality_catalog(&self, part: PartRef) -> Result<QualityCatalog> {
        self.ensure_open()?;
        self.inner.api.quality_catalog(part).await
    }
    pub async fn plan(&self, selection: DownloadSelection) -> Result<StreamPlan> {
        self.ensure_open()?;
        self.inner
            .api
            .plan(selection, &self.inner.config.output_dir)
            .await
    }
    pub async fn enqueue(&self, plan: StreamPlan) -> Result<JobId> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Enqueue(Box::new(plan), tx)).await?;
        receive(rx).await
    }
    pub async fn control(&self, id: JobId, action: JobAction) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Control(id, action, tx)).await?;
        receive(rx).await
    }
    pub fn jobs(&self) -> watch::Receiver<Arc<QueueSnapshot>> {
        self.inner.snapshots.clone()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.inner.events.subscribe()
    }
    pub async fn auth(&self, action: AuthAction) -> Result<AuthSnapshot> {
        self.ensure_open()?;
        let mut auth = self.inner.auth.lock().await;
        self.ensure_open()?;
        auth.apply(action).await
    }
    pub async fn merge(&self, request: MergeRequest) -> Result<JobId> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Merge(request, tx)).await?;
        receive(rx).await
    }
    pub async fn clean(&self, request: CleanupRequest) -> Result<CleanupReport> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Clean(request, tx)).await?;
        receive(rx).await
    }
    pub async fn shutdown(&self, mode: StopMode) -> Result<ShutdownReport> {
        let mut stopped = self.inner.stopped.lock().await;
        if let Some(report) = &*stopped {
            return Ok(report.clone());
        }
        self.inner
            .closing
            .store(true, std::sync::atomic::Ordering::Release);
        let (tx, rx) = oneshot::channel();
        self.inner
            .commands
            .send(Command::Shutdown(mode, tx))
            .await
            .map_err(|_| CoreError::new(ErrorCode::Shutdown, "core is shut down"))?;
        let result = receive(rx).await;
        if let Some(handle) = self.inner.supervisor.lock().await.take() {
            handle
                .await
                .map_err(|_| CoreError::new(ErrorCode::Corruption, "core supervisor failed"))?;
        }
        let report = result?;
        *stopped = Some(report.clone());
        Ok(report)
    }
    fn ensure_open(&self) -> Result<()> {
        if self
            .inner
            .closing
            .load(std::sync::atomic::Ordering::Acquire)
        {
            Err(CoreError::new(ErrorCode::Shutdown, "core is stopping"))
        } else {
            Ok(())
        }
    }
    async fn send(&self, command: Command) -> Result<()> {
        self.ensure_open()?;
        self.inner
            .commands
            .send(command)
            .await
            .map_err(|_| CoreError::new(ErrorCode::Shutdown, "core is shut down"))
    }
}

fn media_client(timeout: std::time::Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(timeout)
        .read_timeout(timeout)
        .user_agent(crate::api::USER_AGENT)
        .build()
        .map_err(|_| CoreError::new(ErrorCode::Transport, "create anonymous media client"))
}
impl Drop for CoreInner {
    fn drop(&mut self) {
        self.abort.cancel();
    }
}
async fn receive<T>(rx: oneshot::Receiver<Result<T>>) -> Result<T> {
    rx.await
        .map_err(|_| CoreError::new(ErrorCode::Shutdown, "core response channel closed"))?
}
struct Active {
    cancel: CancellationToken,
    intent: Option<StopMode>,
}
struct Reducer {
    config: CoreConfig,
    journal: Arc<Journal>,
    records: Vec<JobRecord>,
    revision: u64,
    snapshots: watch::Sender<Arc<QueueSnapshot>>,
    events: broadcast::Sender<CoreEvent>,
    receiver: mpsc::Receiver<Command>,
    sender: mpsc::Sender<Command>,
    api: Arc<Api>,
    client: reqwest::Client,
    sidecar_client: reqwest::Client,
    credentials: Arc<RwLock<CredentialJar>>,
    connections: Arc<Semaphore>,
    merges: Arc<Semaphore>,
    queue: VecDeque<JobId>,
    workers: JoinSet<(JobId, Result<()>)>,
    active: HashMap<JobId, Active>,
    deferred: HashMap<JobId, Vec<Reply<()>>>,
    stopping: Option<Reply<ShutdownReport>>,
    abort: CancellationToken,
    cache: HashMap<JobId, Arc<JobSnapshot>>,
    dirty: HashSet<JobId>,
    failure: Option<CoreError>,
    auth: Arc<Mutex<AuthManager>>,
}
impl Reducer {
    async fn run(mut self) {
        loop {
            self.launch();
            if self.stopping.is_some() && self.workers.is_empty() {
                // Auth store handles may outlive a dropped caller future.
                // Join them while Journal still owns the OS writer lock.
                let auth_result = self.auth.lock().await.shutdown().await;
                let result = self.persist().map(|_| ShutdownReport {
                    completed: self
                        .records
                        .iter()
                        .filter(|r| r.snapshot.phase == JobPhase::Completed)
                        .count() as u64,
                    paused: self
                        .records
                        .iter()
                        .filter(|r| {
                            matches!(r.snapshot.phase, JobPhase::Paused | JobPhase::Cancelled)
                        })
                        .count() as u64,
                    failed: self
                        .records
                        .iter()
                        .filter(|r| {
                            matches!(
                                r.snapshot.phase,
                                JobPhase::Failed | JobPhase::NeedsAttention
                            )
                        })
                        .count() as u64,
                });
                let result = result.and_then(|report| auth_result.map(|_| report));
                if let Some(reply) = self.stopping.take() {
                    let _ = reply.send(result);
                }
                break;
            }
            tokio::select! {
                biased;
                _=self.abort.cancelled(),if self.stopping.is_none()=>{self.stop_all(StopMode::Pause);let(tx,_)=oneshot::channel();self.stopping=Some(tx);},
                command=self.receiver.recv()=>match command {Some(c)=>self.command(c),None=>{self.stop_all(StopMode::Pause);let(tx,_)=oneshot::channel();self.stopping=Some(tx);}},
                result=self.workers.join_next(),if !self.workers.is_empty()=>{
                    if let Some(Ok((id,result)))=result {self.finished(id,result);}
                    else {self.stop_all(StopMode::Pause);for r in &mut self.records{if r.snapshot.phase.is_active(){r.snapshot.phase=JobPhase::Failed;r.snapshot.error=Some(CoreError::new(ErrorCode::Corruption,"job supervisor failed"));self.dirty.insert(r.snapshot.id);}}let _=self.persist();}
                },
            }
        }
    }
    fn writable(&self) -> Result<()> {
        if let Some(error) = &self.failure {
            Err(error.clone())
        } else if self.config.state_mode == StateMode::ReadOnly {
            Err(CoreError::new(
                ErrorCode::ReadOnly,
                "read-only state inspection",
            ))
        } else if self.stopping.is_some() {
            Err(CoreError::new(ErrorCode::Shutdown, "core is stopping"))
        } else {
            Ok(())
        }
    }
    fn index(&self, id: JobId) -> Result<usize> {
        self.records
            .iter()
            .position(|r| r.snapshot.id == id)
            .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "unknown job"))
    }
    fn persist(&mut self) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "queue revision overflow"))?;
        for record in &mut self.records {
            if self.dirty.contains(&record.snapshot.id) {
                record.snapshot.revision = self.revision;
            }
        }
        if self.config.state_mode == StateMode::ReadWrite {
            if let Err(error) =
                self.journal
                    .save(self.revision, &mut self.records, Some(&self.dirty))
            {
                // A failed durable transition poisons admission until a new
                // writer session recovers the last acknowledged checkpoint.
                self.failure = Some(error.clone());
                self.queue.clear();
                for active in self.active.values_mut() {
                    active.cancel.cancel();
                }
                for record in &mut self.records {
                    if self.dirty.contains(&record.snapshot.id)
                        || self.active.contains_key(&record.snapshot.id)
                    {
                        record.snapshot.phase = JobPhase::Failed;
                        record.snapshot.error = Some(error.clone());
                        record.snapshot.revision = self.revision;
                    }
                }
                self.publish();
                return Err(error);
            }
        }
        self.dirty.clear();
        self.publish();
        Ok(())
    }
    fn publish(&mut self) {
        let jobs = self
            .records
            .iter()
            .map(|record| {
                let value = self
                    .cache
                    .entry(record.snapshot.id)
                    .or_insert_with(|| Arc::new(record.snapshot.clone()));
                if value.revision != record.snapshot.revision {
                    *value = Arc::new(record.snapshot.clone());
                }
                value.clone()
            })
            .collect();
        self.snapshots.send_replace(Arc::new(QueueSnapshot {
            schema_version: 1,
            revision: self.revision,
            jobs,
        }));
    }
    fn notify(&self, id: JobId, phase: JobPhase) {
        let _ = self.events.send(CoreEvent {
            sequence: self.revision,
            job: Some(id),
            phase: Some(phase),
            message: "job transition".into(),
        });
    }
    fn command(&mut self, command: Command) {
        match command {
            Command::Enqueue(plan, reply) => {
                let result = self.enqueue(*plan);
                let _ = reply.send(result);
            }
            Command::Control(id, action, reply) => {
                let stopping = matches!(action, JobAction::Pause | JobAction::Cancel);
                if stopping && self.active.contains_key(&id) {
                    if self.deferred.values().map(Vec::len).sum::<usize>() >= 128 {
                        let _ = reply.send(Err(CoreError::new(
                            ErrorCode::Busy,
                            "too many pending controls",
                        )));
                        return;
                    }
                    let committing = self
                        .index(id)
                        .is_ok_and(|i| self.records[i].snapshot.phase == JobPhase::Committing);
                    if committing {
                        self.deferred.entry(id).or_default().push(reply);
                        return;
                    }
                    match self.control(id, action) {
                        Ok(()) => {
                            self.deferred.entry(id).or_default().push(reply);
                        }
                        Err(e) => {
                            let _ = reply.send(Err(e));
                        }
                    }
                    return;
                }
                let result = self.control(id, action);
                let _ = reply.send(result);
            }
            Command::Merge(request, reply) => {
                let result = self.merge(request);
                let _ = reply.send(result);
            }
            Command::Clean(request, reply) => {
                let result = self.clean(request);
                let _ = reply.send(result);
            }
            Command::Shutdown(mode, reply) => {
                if self.stopping.is_some() {
                    let _ = reply.send(Err(CoreError::new(
                        ErrorCode::Shutdown,
                        "shutdown already requested",
                    )));
                } else {
                    self.stop_all(mode);
                    self.stopping = Some(reply);
                }
            }
            Command::Update(id, update, reply) => {
                let result = self.update(id, update);
                let _ = reply.send(result);
            }
            Command::Progress(id, bytes) => {
                if self.failure.is_none() {
                    if let Ok(i) = self.index(id) {
                        if self.records[i].snapshot.downloaded == bytes {
                            return;
                        }
                        self.revision = self.revision.saturating_add(1);
                        self.records[i].snapshot.downloaded = bytes;
                        self.records[i].snapshot.revision = self.revision;
                        self.publish();
                        let _ = self.events.send(CoreEvent {
                            sequence: self.revision,
                            job: Some(id),
                            phase: Some(self.records[i].snapshot.phase),
                            message: "job progress".into(),
                        });
                    }
                }
            }
        }
    }
    fn enqueue(&mut self, mut plan: StreamPlan) -> Result<JobId> {
        self.writable()?;
        if plan.schema_version != 1
            || !plan.destination.is_absolute()
            || plan.video.part != plan.part
            || plan.audio.as_ref().is_some_and(|a| a.part != plan.part)
        {
            return Err(CoreError::new(
                ErrorCode::InvalidInput,
                "invalid stream plan",
            ));
        }
        // StreamPlan is an editable DTO. Never trust a stale caller-provided
        // fingerprint to turn a changed exact representation into a skip.
        plan.fingerprint = crate::api::plan_fingerprint(&plan)?;
        for record in &mut self.records {
            if record.snapshot.plan.as_ref().is_some_and(|p| {
                p.fingerprint == plan.fingerprint && p.destination == plan.destination
            }) {
                if record.snapshot.phase == JobPhase::Completed {
                    if record
                        .snapshot
                        .plan
                        .as_ref()
                        .is_some_and(|old| old.sidecars != plan.sidecars)
                    {
                        record.components_pending = true;
                    }
                    record.snapshot.plan = Some(Arc::new(plan));
                    record.snapshot.phase = JobPhase::Queued;
                    record.snapshot.skipped = false;
                    let id = record.snapshot.id;
                    self.dirty.insert(id);
                    self.persist()?;
                    self.queue.push_back(id);
                    return Ok(id);
                }
                return Err(CoreError::new(
                    ErrorCode::Conflict,
                    "matching job exists; resume or retry it",
                ));
            }
        }
        if self.records.len() >= 1024 {
            return Err(CoreError::new(
                ErrorCode::Busy,
                "queue history capacity reached; remove old jobs",
            ));
        }
        state::reject_symlink(&plan.destination)?;
        if plan.destination.try_exists().map_err(CoreError::io)? {
            return Err(CoreError::new(
                ErrorCode::Conflict,
                "output already exists without matching completion",
            ));
        }
        let id = JobId::new();
        self.journal.job_dir(id)?;
        self.records.push(JobRecord::planned(id, plan));
        self.dirty.insert(id);
        self.persist()?;
        self.queue.push_back(id);
        Ok(id)
    }
    fn control(&mut self, id: JobId, action: JobAction) -> Result<()> {
        self.writable()?;
        let i = self.index(id)?;
        let phase = self.records[i].snapshot.phase;
        self.dirty.insert(id);
        match action {
            JobAction::Pause | JobAction::Cancel => {
                if phase == JobPhase::Completed {
                    return Err(CoreError::new(ErrorCode::Conflict, "job already completed"));
                }
                let mode = if matches!(action, JobAction::Pause) {
                    StopMode::Pause
                } else {
                    StopMode::Cancel
                };
                self.queue.retain(|j| *j != id);
                if let Some(active) = self.active.get_mut(&id) {
                    active.intent = Some(mode);
                    active.cancel.cancel();
                    self.records[i].snapshot.phase = if matches!(mode, StopMode::Pause) {
                        JobPhase::Pausing
                    } else {
                        JobPhase::Cancelling
                    };
                } else {
                    self.records[i].snapshot.phase = if matches!(mode, StopMode::Pause) {
                        JobPhase::Paused
                    } else {
                        JobPhase::Cancelled
                    };
                }
            }
            JobAction::Resume | JobAction::Retry => {
                if phase.is_active() {
                    return Err(CoreError::new(ErrorCode::Conflict, "job already active"));
                }
                if phase == JobPhase::Completed
                    && self.records[i]
                        .snapshot
                        .components
                        .iter()
                        .all(|c| c.error.is_none())
                {
                    return Err(CoreError::new(ErrorCode::Conflict, "job already completed"));
                }
                self.records[i].snapshot.phase = JobPhase::Queued;
                self.records[i].snapshot.error = None;
                self.records[i].snapshot.skipped = false;
                self.queue.push_back(id);
            }
            JobAction::Remove { cleanup } => {
                if phase.is_active() {
                    return Err(CoreError::new(
                        ErrorCode::Conflict,
                        "cannot remove active job",
                    ));
                }
                if cleanup {
                    let request = CleanupRequest {
                        job: Some(id),
                        abandoned: false,
                        include_resumable: true,
                        dry_run: false,
                    };
                    let result = self.journal.clean(&self.records[i], &request);
                    if let Some(error) = result.errors.into_iter().next() {
                        return Err(error);
                    }
                }
                self.records.remove(i);
                self.cache.remove(&id);
            }
        }
        self.persist()
    }
    fn merge(&mut self, request: MergeRequest) -> Result<JobId> {
        self.writable()?;
        match request {
            MergeRequest::Managed(id) => {
                let i = self.index(id)?;
                if self.records[i].snapshot.phase.is_active() {
                    return Err(CoreError::new(ErrorCode::Conflict, "job already active"));
                }
                let plan = self.records[i].snapshot.plan.as_mut().ok_or_else(|| {
                    CoreError::new(ErrorCode::InvalidInput, "not a managed download")
                })?;
                Arc::make_mut(plan).merge = MergePolicy::Automatic;
                self.records[i].snapshot.phase = JobPhase::Queued;
                self.records[i].snapshot.error = None;
                self.dirty.insert(id);
                self.persist()?;
                self.queue.push_back(id);
                Ok(id)
            }
            MergeRequest::Local {
                video,
                audio,
                output,
            } => {
                if self.records.len() >= 1024 {
                    return Err(CoreError::new(
                        ErrorCode::Busy,
                        "queue history capacity reached; remove old jobs",
                    ));
                }
                if !video.is_absolute()
                    || audio.as_ref().is_some_and(|p| !p.is_absolute())
                    || !output.is_absolute()
                {
                    return Err(CoreError::new(
                        ErrorCode::InvalidInput,
                        "manual merge paths must be absolute",
                    ));
                }
                state::reject_symlink(&video)?;
                if let Some(audio) = &audio {
                    state::reject_symlink(audio)?;
                }
                if output.try_exists().map_err(CoreError::io)? {
                    return Err(CoreError::new(
                        ErrorCode::Conflict,
                        "manual output already exists",
                    ));
                }
                let id = JobId::new();
                let record = JobRecord {
                    schema_version: 1,
                    checkpoint_revision: 0,
                    components_pending: false,
                    snapshot: JobSnapshot {
                        id,
                        revision: 0,
                        plan: None,
                        phase: JobPhase::Queued,
                        downloaded: 0,
                        total: None,
                        output: None,
                        skipped: false,
                        error: None,
                        components: Vec::new(),
                    },
                    resources: Vec::new(),
                    completion: None,
                    local: Some(LocalMerge {
                        video,
                        audio,
                        output,
                    }),
                };
                self.journal.job_dir(id)?;
                self.records.push(record);
                self.dirty.insert(id);
                self.persist()?;
                self.queue.push_back(id);
                Ok(id)
            }
        }
    }
    fn clean(&self, request: CleanupRequest) -> Result<CleanupReport> {
        self.writable()?;
        if request.job.is_none() && !request.abandoned {
            return Err(CoreError::new(
                ErrorCode::InvalidInput,
                "cleanup requires a job or abandoned selection",
            ));
        }
        let mut result = CleanupReport::default();
        for record in &self.records {
            if request.job.is_some_and(|id| id != record.snapshot.id) {
                continue;
            }
            if request.abandoned
                && matches!(
                    record.snapshot.phase,
                    JobPhase::Completed | JobPhase::Downloaded
                )
            {
                continue;
            }
            let report = self.journal.clean(record, &request);
            result.paths.extend(report.paths);
            result.bytes += report.bytes;
            result.errors.extend(report.errors);
        }
        Ok(result)
    }
    fn update(&mut self, id: JobId, update: Update) -> Result<()> {
        let i = self.index(id)?;
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        self.dirty.insert(id);
        if self
            .active
            .get(&id)
            .is_some_and(|a| a.cancel.is_cancelled())
            && self.records[i].snapshot.phase != JobPhase::Committing
        {
            return Err(CoreError::new(
                ErrorCode::Cancelled,
                "job stopping before commit",
            ));
        }
        match update {
            Update::Phase(phase) => self.records[i].snapshot.phase = phase,
            Update::Resources(resources) => self.records[i].resources = resources,
            Update::Components(components) => {
                self.records[i].components_pending = components.iter().any(|c| c.error.is_some());
                self.records[i].snapshot.components = components;
            }
            Update::Skipped => self.records[i].snapshot.skipped = true,
            Update::Commit(completion) => {
                self.records[i].completion = Some(completion);
                self.records[i].snapshot.phase = JobPhase::Committing;
            }
            Update::Promoted => {
                let completion = self.records[i].completion.as_mut().ok_or_else(|| {
                    CoreError::new(ErrorCode::Corruption, "promotion proof missing")
                })?;
                completion.promoted = true;
                self.records[i].snapshot.output = self.records[i]
                    .snapshot
                    .plan
                    .as_ref()
                    .map(|p| p.destination.clone())
                    .or_else(|| self.records[i].local.as_ref().map(|l| l.output.clone()));
            }
        }
        self.persist()?;
        self.notify(id, self.records[i].snapshot.phase);
        Ok(())
    }
    fn stop_all(&mut self, mode: StopMode) {
        self.queue.clear();
        for record in &mut self.records {
            if record.snapshot.phase == JobPhase::Queued {
                self.dirty.insert(record.snapshot.id);
                record.snapshot.phase = if matches!(mode, StopMode::Pause) {
                    JobPhase::Paused
                } else {
                    JobPhase::Cancelled
                };
            }
        }
        for (id, active) in &mut self.active {
            if self
                .records
                .iter()
                .any(|r| r.snapshot.id == *id && r.snapshot.phase == JobPhase::Committing)
            {
                continue;
            }
            active.intent = Some(mode);
            active.cancel.cancel();
            if let Some(record) = self.records.iter_mut().find(|r| r.snapshot.id == *id) {
                record.snapshot.phase = if matches!(mode, StopMode::Pause) {
                    JobPhase::Pausing
                } else {
                    JobPhase::Cancelling
                };
                self.dirty.insert(*id);
            }
        }
        let _ = self.persist();
    }
    fn launch(&mut self) {
        if self.stopping.is_some() || self.failure.is_some() {
            return;
        }
        while self.workers.len() < self.config.max_jobs {
            let Some(id) = self.queue.pop_front() else {
                break;
            };
            let Ok(i) = self.index(id) else { continue };
            let cancel = self.abort.child_token();
            self.active.insert(
                id,
                Active {
                    cancel: cancel.clone(),
                    intent: None,
                },
            );
            let context = Worker {
                record: self.records[i].clone(),
                journal: self.journal.clone(),
                commands: self.sender.clone(),
                api: self.api.clone(),
                client: self.client.clone(),
                sidecar_client: self.sidecar_client.clone(),
                credentials: self.credentials.clone(),
                connections: self.connections.clone(),
                merges: self.merges.clone(),
                config: self.config.clone(),
                cancel,
            };
            self.workers.spawn(async move {
                let result = std::panic::AssertUnwindSafe(context.run())
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        Err(CoreError::new(ErrorCode::Corruption, "job worker panicked"))
                    });
                (id, result)
            });
        }
    }
    fn finished(&mut self, id: JobId, result: Result<()>) {
        let active = self.active.remove(&id);
        let Ok(i) = self.index(id) else { return };
        match result {
            Ok(()) => {
                let manual = self.records[i]
                    .snapshot
                    .plan
                    .as_ref()
                    .is_some_and(|p| p.merge == MergePolicy::Manual)
                    && self.records[i].completion.is_none();
                self.records[i].snapshot.phase = if manual {
                    JobPhase::Downloaded
                } else {
                    JobPhase::Completed
                };
            }
            Err(error) if error.code == ErrorCode::Cancelled => {
                self.records[i].snapshot.phase =
                    if matches!(active.and_then(|a| a.intent), Some(StopMode::Cancel)) {
                        JobPhase::Cancelled
                    } else {
                        JobPhase::Paused
                    };
            }
            Err(error) => {
                self.records[i].snapshot.phase = if matches!(
                    error.code,
                    ErrorCode::LoginRequired
                        | ErrorCode::EntitlementRequired
                        | ErrorCode::VerificationRequired
                        | ErrorCode::RepresentationChanged
                        | ErrorCode::Unavailable
                ) {
                    JobPhase::NeedsAttention
                } else {
                    JobPhase::Failed
                };
                self.records[i].snapshot.error = Some(error);
            }
        }
        if let Some(error) = &self.failure {
            self.records[i].snapshot.phase = JobPhase::Failed;
            self.records[i].snapshot.error = Some(error.clone());
        }
        self.dirty.insert(id);
        if self.records[i]
            .completion
            .as_ref()
            .is_some_and(|c| c.promoted)
        {
            self.records[i].snapshot.output = self.records[i]
                .snapshot
                .plan
                .as_ref()
                .map(|p| p.destination.clone())
                .or_else(|| self.records[i].local.as_ref().map(|l| l.output.clone()));
        }
        if let Err(error) = self.persist() {
            self.records[i].snapshot.phase = JobPhase::Failed;
            self.records[i].snapshot.error = Some(error);
            self.publish();
        }
        self.notify(id, self.records[i].snapshot.phase);
        if let Some(waiters) = self.deferred.remove(&id) {
            for reply in waiters {
                let result = if self.records[i].snapshot.phase == JobPhase::Completed {
                    Err(CoreError::new(
                        ErrorCode::Conflict,
                        "job completed before stop",
                    ))
                } else {
                    self.records[i].snapshot.error.clone().map_or(Ok(()), Err)
                };
                let _ = reply.send(result);
            }
        }
    }
}
struct Worker {
    record: JobRecord,
    journal: Arc<Journal>,
    commands: mpsc::Sender<Command>,
    api: Arc<Api>,
    client: reqwest::Client,
    sidecar_client: reqwest::Client,
    credentials: Arc<RwLock<CredentialJar>>,
    connections: Arc<Semaphore>,
    merges: Arc<Semaphore>,
    config: CoreConfig,
    cancel: CancellationToken,
}
impl Worker {
    fn id(&self) -> JobId {
        self.record.snapshot.id
    }
    async fn update(&self, update: Update) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Update(self.id(), update, tx))
            .await
            .map_err(|_| CoreError::new(ErrorCode::Shutdown, "job reducer unavailable"))?;
        receive(rx).await
    }
    async fn phase(&self, phase: JobPhase) -> Result<()> {
        self.update(Update::Phase(phase)).await
    }
    async fn run(mut self) -> Result<()> {
        // A completed video survives a failed sidecar; retry only requested components.
        if self.record.completion.is_some() {
            self.phase(JobPhase::Verifying).await?;
            let record = self.record.clone();
            let cancel = self.cancel.clone();
            let verified = tokio::task::spawn_blocking(move || {
                state::verify_completion_cancel(&record, &cancel)
            })
            .await
            .map_err(|_| {
                CoreError::new(
                    ErrorCode::Corruption,
                    "completion verification worker failed",
                )
            })??;
            if verified {
                if self.record.completion.as_ref().is_some_and(|c| !c.promoted) {
                    self.update(Update::Promoted).await?;
                }
                let mut retry = self.record.components_pending
                    || self
                        .record
                        .snapshot
                        .components
                        .iter()
                        .any(|c| c.error.is_some());
                if !retry {
                    let paths: Vec<_> = self
                        .record
                        .snapshot
                        .components
                        .iter()
                        .filter_map(|component| component.path.clone())
                        .collect();
                    if !paths.is_empty() {
                        let cancel = self.cancel.clone();
                        retry = !tokio::task::spawn_blocking(move || -> Result<bool> {
                            for path in paths {
                                if !state::verify_component(&path, &cancel)? {
                                    return Ok(false);
                                }
                            }
                            Ok(true)
                        })
                        .await
                        .map_err(|_| {
                            CoreError::new(
                                ErrorCode::Corruption,
                                "component verification worker failed",
                            )
                        })??;
                    }
                }
                if retry {
                    if let Some(plan) = &self.record.snapshot.plan {
                        let components = self.sidecars(plan).await;
                        self.update(Update::Components(components.clone())).await?;
                        if components.iter().any(|c| c.error.is_some()) {
                            return Err(CoreError::new(
                                ErrorCode::Transport,
                                "requested sidecar component failed",
                            ));
                        }
                    }
                } else {
                    self.update(Update::Skipped).await?;
                }
                if let Some(plan) = &self.record.snapshot.plan {
                    if !plan.keep_temp {
                        let mut done = self.record.clone();
                        done.snapshot.phase = JobPhase::Completed;
                        let report = self.journal.clean(
                            &done,
                            &CleanupRequest {
                                job: Some(self.id()),
                                abandoned: false,
                                include_resumable: false,
                                dry_run: false,
                            },
                        );
                        if let Some(error) = report.errors.into_iter().next() {
                            return Err(error);
                        }
                    }
                }
                state::clean_staging(&self.record, false)?;
                return Ok(());
            }
            if self
                .record
                .snapshot
                .output
                .as_ref()
                .is_some_and(|p| p.exists())
            {
                return Err(CoreError::new(
                    ErrorCode::Corruption,
                    "completed output failed identity verification",
                ));
            }
        }
        self.phase(JobPhase::Resolving).await?;
        let (inputs, topology, destination) = if let Some(local) = &self.record.local {
            let mut paths = vec![local.video.clone()];
            if let Some(audio) = &local.audio {
                paths.push(audio.clone());
            }
            let topology = if paths.len() == 2 {
                StreamTopology::Dash
            } else {
                StreamTopology::Direct
            };
            (paths, topology, local.output.clone())
        } else {
            let plan = self
                .record
                .snapshot
                .plan
                .as_ref()
                .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "job has no plan"))?
                .clone();
            let locators = self.api.locators(&plan).await?;
            let resources: Vec<ResourceRecord> = locators
                .iter()
                .enumerate()
                .map(|(i, l)| ResourceRecord {
                    name: format!("resource-{i}.media"),
                    identity: l.identity.clone(),
                })
                .collect();
            if !self.record.resources.is_empty()
                && (self.record.resources.len() != resources.len()
                    || self
                        .record
                        .resources
                        .iter()
                        .zip(&resources)
                        .any(|(old, new)| old.identity != new.identity))
            {
                return Err(CoreError::new(
                    ErrorCode::RepresentationChanged,
                    "resource identity changed",
                ));
            }
            self.update(Update::Resources(resources.clone())).await?;
            self.record.resources = resources;
            self.phase(JobPhase::Downloading).await?;
            let mut paths = Vec::new();
            let mut completed_bytes = 0u64;
            for (index, locator) in locators.iter().enumerate() {
                let path = self
                    .journal
                    .owned_file(self.id(), &self.record.resources[index].name)?;
                let mut checkpoint = crate::download::load_checkpoint(&path)?;
                let mut attempt = 0;
                loop {
                    let fresh;
                    let current = if attempt == 0 {
                        locator
                    } else {
                        fresh = self.api.locators(&plan).await?;
                        fresh
                            .get(index)
                            .filter(|l| l.identity == locator.identity)
                            .ok_or_else(|| {
                                CoreError::new(
                                    ErrorCode::RepresentationChanged,
                                    "renewed locator identity changed",
                                )
                            })?
                    };
                    let permit = tokio::select! {
                        _=self.cancel.cancelled()=>return Err(CoreError::new(ErrorCode::Cancelled,"download cancelled")),
                        permit=self.connections.acquire()=>match permit {
                            Ok(permit)=>permit,
                            Err(_)=>return Err(CoreError::new(ErrorCode::Shutdown,"connection limiter closed")),
                        }
                    };
                    let sender = self.commands.clone();
                    let id = self.id();
                    let base = completed_bytes;
                    let result = crate::download::transfer(
                        &self.client,
                        current,
                        &path,
                        &mut checkpoint,
                        crate::download::TransferOptions {
                            cancel: &self.cancel,
                            rate_limit: self.config.rate_limit,
                        },
                        &self.config.retry,
                        move |bytes| {
                            let _ =
                                sender.try_send(Command::Progress(id, base.saturating_add(bytes)));
                        },
                    )
                    .await;
                    drop(permit);
                    match result {
                        Ok(()) => break,
                        Err(error)
                            if error.code == ErrorCode::Transport
                                && error.retryable
                                && attempt + 1 < self.config.retry.attempts =>
                        {
                            attempt += 1;
                        }
                        Err(error) => return Err(error),
                    }
                }
                completed_bytes = completed_bytes.saturating_add(checkpoint.durable_bytes);
                paths.push(path);
                self.commands
                    .send(Command::Progress(self.id(), completed_bytes))
                    .await
                    .map_err(|_| CoreError::new(ErrorCode::Shutdown, "job reducer unavailable"))?;
            }
            if plan.merge == MergePolicy::Manual {
                let components = self.sidecars(&plan).await;
                self.update(Update::Components(components.clone())).await?;
                if components.iter().any(|c| c.error.is_some()) {
                    return Err(CoreError::new(
                        ErrorCode::Transport,
                        "requested sidecar component failed",
                    ));
                }
                return Ok(());
            }
            self.phase(JobPhase::Downloaded).await?;
            (paths, plan.topology, plan.destination.clone())
        };
        let staging = state::prepare_staging(&self.record)?;
        // Restart only an owned incomplete remux; verified source inputs stay.
        if staging.try_exists().map_err(CoreError::io)? {
            tokio::fs::remove_file(&staging)
                .await
                .map_err(CoreError::io)?;
        }
        self.phase(JobPhase::Remuxing).await?;
        let _merge_permit = tokio::select! {_=self.cancel.cancelled()=>return Err(CoreError::new(ErrorCode::Cancelled,"remux cancelled")),permit=self.merges.acquire()=>permit.map_err(|_|CoreError::new(ErrorCode::Shutdown,"remux limiter closed"))?};
        let input_copy = inputs.clone();
        let staging_copy = staging.clone();
        let cancel = self.cancel.clone();
        let probe = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            run_native_gate(&input_copy);
            crate::remux::remux(&input_copy, topology, &staging_copy, &cancel)
        })
        .await
        .map_err(|_| CoreError::new(ErrorCode::Remux, "native remux worker failed"))??;
        drop(_merge_permit);
        if let Some(plan) = &self.record.snapshot.plan {
            crate::remux::validate_plan(&probe, plan)?;
        }
        self.phase(JobPhase::Verifying).await?;
        let hash_path = staging.clone();
        let cancel = self.cancel.clone();
        let (size, sha256) =
            tokio::task::spawn_blocking(move || state::file_digest_cancel(&hash_path, &cancel))
                .await
                .map_err(|_| {
                    CoreError::new(ErrorCode::Corruption, "output verification worker failed")
                })??;
        let fingerprint = self
            .record
            .snapshot
            .plan
            .as_ref()
            .map(|p| p.fingerprint.clone())
            .unwrap_or_else(|| format!("manual:{}", self.id()));
        let components = if let Some(plan) = &self.record.snapshot.plan {
            self.sidecars(plan).await
        } else {
            Vec::new()
        };
        self.update(Update::Components(components.clone())).await?;
        // Reducer checks cancellation and persists recovery proof before gate entry.
        self.update(Update::Commit(Completion {
            sha256,
            size,
            fingerprint,
            promoted: false,
        }))
        .await?;
        state::promote(&staging, &destination)?;
        self.update(Update::Promoted).await?;
        // No cancellation checks after entering Committing. Sidecars finished
        // before gate entry, so this critical section contains only promotion.
        if components.iter().any(|c| c.error.is_some()) {
            return Err(CoreError::new(
                ErrorCode::Transport,
                "requested sidecar component failed",
            ));
        }
        if let Some(plan) = &self.record.snapshot.plan {
            if !plan.keep_temp {
                let mut done = self.record.clone();
                done.snapshot.phase = JobPhase::Completed;
                let report = self.journal.clean(
                    &done,
                    &CleanupRequest {
                        job: Some(self.id()),
                        abandoned: false,
                        include_resumable: false,
                        dry_run: false,
                    },
                );
                if let Some(error) = report.errors.into_iter().next() {
                    return Err(error);
                }
            }
        }
        state::clean_staging(&self.record, false)?;
        Ok(())
    }
    async fn sidecars(&self, plan: &StreamPlan) -> Vec<ComponentResult> {
        if matches!(plan.sidecars.subtitles, SubtitleSelection::None) && !plan.sidecars.danmaku {
            return Vec::new();
        }
        let permit = tokio::select! {
            _=self.cancel.cancelled()=>Err(CoreError::new(ErrorCode::Cancelled,"sidecars cancelled")),
            permit=self.connections.acquire()=>permit.map_err(|_|CoreError::new(ErrorCode::Shutdown,"connection limiter closed")),
        };
        let _permit = match permit {
            Ok(permit) => permit,
            Err(error) => {
                return vec![ComponentResult {
                    name: "sidecars".into(),
                    path: None,
                    error: Some(error),
                    omissions: 0,
                }]
            }
        };
        crate::sidecar::download(
            &plan.part,
            &plan.sidecars,
            &plan.destination,
            &self.sidecar_client,
            &self.credentials,
            &self.cancel,
        )
        .await
    }
}

#[cfg(test)]
type NativeGate = (oneshot::Sender<()>, Arc<std::sync::Barrier>);
#[cfg(test)]
static NATIVE_GATES: std::sync::LazyLock<
    std::sync::Mutex<HashMap<std::path::PathBuf, NativeGate>>,
> = std::sync::LazyLock::new(Default::default);
#[cfg(test)]
fn run_native_gate(inputs: &[std::path::PathBuf]) {
    let gate = inputs
        .first()
        .and_then(|path| NATIVE_GATES.lock().ok()?.remove(path));
    if let Some((entered, release)) = gate {
        let _ = entered.send(());
        release.wait();
    }
}
#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_retains_writer_lock_until_owned_blocking_worker_joins() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("source.media");
        std::fs::write(&input, b"retained local input").unwrap();
        let config = CoreConfig::new(root.path().join("state"), root.path().join("output"));
        let core = Core::new(config.clone(), Handle::current()).unwrap();
        let (entered, started) = oneshot::channel();
        let release = Arc::new(std::sync::Barrier::new(2));
        NATIVE_GATES
            .lock()
            .unwrap()
            .insert(input.clone(), (entered, release.clone()));
        let id = core
            .merge(MergeRequest::Local {
                video: input.clone(),
                audio: None,
                output: root.path().join("video.mp4"),
            })
            .await
            .unwrap();
        started.await.unwrap();
        // Exercise the same reducer command used by transfer progress while
        // a real owned worker is nonterminal. No source-text assertions.
        let before = core
            .jobs()
            .borrow()
            .jobs
            .iter()
            .find(|job| job.id == id)
            .cloned();
        let durable_before = Journal::open(&config.state_dir, StateMode::ReadOnly)
            .and_then(|journal| journal.load());
        let mut progress = core.jobs();
        let mut events = core.subscribe();
        let sent = core
            .inner
            .commands
            .send(Command::Progress(id, 16_384))
            .await;
        let changed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let current = progress
                    .borrow()
                    .jobs
                    .iter()
                    .find(|job| job.id == id)
                    .cloned();
                if current.as_ref().is_some_and(|job| job.downloaded == 16_384) {
                    break current;
                }
                if progress.changed().await.is_err() {
                    break None;
                }
            }
        })
        .await;
        let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv()).await;
        let durable_after = Journal::open(&config.state_dir, StateMode::ReadOnly)
            .and_then(|journal| journal.load());
        let shutdown = core.shutdown(StopMode::Pause);
        tokio::pin!(shutdown);
        let shutdown_pending = futures_util::poll!(&mut shutdown).is_pending();
        let mut snapshots = core.jobs();
        let stopped_phase = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let phase = snapshots.borrow().jobs.first().map(|job| job.phase);
                if matches!(
                    phase,
                    Some(
                        JobPhase::Pausing
                            | JobPhase::Paused
                            | JobPhase::Failed
                            | JobPhase::Completed
                    )
                ) {
                    break phase;
                }
                snapshots.changed().await.unwrap();
            }
        })
        .await;
        let contender = Core::new(config.clone(), Handle::current());
        if let Ok(other) = &contender {
            other.shutdown(StopMode::Pause).await.unwrap();
        }
        release.wait();
        let result = shutdown.await;
        sent.unwrap();
        let before = before.unwrap();
        let changed = changed.unwrap().unwrap();
        let event = event.unwrap().unwrap();
        assert_eq!(event.message, "job progress");
        assert_eq!(event.job, Some(id));
        assert_eq!(event.sequence, changed.revision);
        assert_eq!(event.phase, Some(changed.phase));
        assert!(changed.revision > before.revision);
        assert!(!Arc::ptr_eq(&before, &changed));
        assert!(!matches!(
            changed.phase,
            JobPhase::Completed | JobPhase::Failed
        ));
        let durable_before = durable_before.unwrap();
        let durable_after = durable_after.unwrap();
        assert_eq!(durable_before.0, durable_after.0);
        assert_eq!(
            durable_before.1[0].checkpoint_revision,
            durable_after.1[0].checkpoint_revision
        );
        assert!(shutdown_pending);
        assert_eq!(stopped_phase.unwrap(), Some(JobPhase::Pausing));
        assert!(matches!(contender,Err(error) if error.code==ErrorCode::Busy));
        result.unwrap();
        assert_eq!(std::fs::read(input).unwrap(), b"retained local input");
        assert!(!root.path().join("video.mp4").exists());
        let replacement = Core::new(config, Handle::current()).unwrap();
        replacement.shutdown(StopMode::Pause).await.unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn changed_component_policy_reuses_verified_video_then_cleans_owned_inputs() {
        let root = tempfile::tempdir().unwrap();
        let config = CoreConfig::new(root.path().join("state"), root.path().join("output"));
        let journal = Journal::open(&config.state_dir, StateMode::ReadWrite).unwrap();
        let output = root.path().join("completed.mp4");
        // This journal fixture tests digest-backed reuse, not media decoding.
        std::fs::write(&output, b"previously committed output bytes").unwrap();
        let part = PartRef {
            content: ContentRef::Video(Aid::new(1).unwrap()),
            aid: Aid::new(1).unwrap(),
            cid: Cid::new(2).unwrap(),
            episode: None,
        };
        let video = RepresentationKey {
            part: part.clone(),
            id: 80,
            quality: 80,
            codec: "avc1".into(),
            width: Some(1920),
            height: Some(1080),
            fps: None,
            dynamic_range: None,
            color: None,
            bandwidth: 1,
            audio: false,
            sample_rate: None,
            channels: None,
            language: None,
        };
        let mut plan = StreamPlan {
            schema_version: 1,
            fingerprint: String::new(),
            part,
            title: "fixture".into(),
            video,
            audio: None,
            topology: StreamTopology::Direct,
            segments: vec![],
            destination: output.clone(),
            merge: MergePolicy::Automatic,
            sidecars: SidecarPolicy::default(),
            keep_temp: false,
            catalog_revision: 1,
        };
        plan.fingerprint = crate::api::plan_fingerprint(&plan).unwrap();
        let id = JobId::new();
        let mut record = JobRecord::planned(id, plan.clone());
        record.components_pending = false;
        record.snapshot.phase = JobPhase::Completed;
        record.snapshot.output = Some(output.clone());
        let (size, sha256) = state::file_digest_cancel(&output, &CancellationToken::new()).unwrap();
        record.completion = Some(Completion {
            size,
            sha256,
            fingerprint: plan.fingerprint.clone(),
            promoted: true,
        });
        record.resources.push(ResourceRecord {
            name: "resource-0.media".into(),
            identity: "owned-fixture".into(),
        });
        let owned = journal.owned_file(id, "resource-0.media").unwrap();
        std::fs::write(&owned, b"retained while sidecars pending").unwrap();
        journal.save(1, &mut [record], None).unwrap();
        drop(journal);
        let core = Core::new(config, Handle::current()).unwrap();
        plan.sidecars = SidecarPolicy {
            subtitles: SubtitleSelection::None,
            danmaku: false,
        };
        plan.title = "updated display title".into();
        plan.catalog_revision = 900;
        for skipped in [false, true] {
            assert_eq!(core.enqueue(plan.clone()).await.unwrap(), id);
            let mut jobs = core.jobs();
            let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let job = jobs
                        .borrow()
                        .jobs
                        .iter()
                        .find(|job| job.id == id)
                        .unwrap()
                        .clone();
                    if matches!(
                        job.phase,
                        JobPhase::Completed | JobPhase::Failed | JobPhase::NeedsAttention
                    ) {
                        break job;
                    }
                    jobs.changed().await.unwrap();
                }
            })
            .await
            .unwrap();
            assert_eq!(result.phase, JobPhase::Completed);
            assert_eq!(result.skipped, skipped);
            plan.keep_temp = true;
            plan.merge = MergePolicy::Manual;
            plan.catalog_revision += 1;
        }
        let mut changed_quality = plan.clone();
        changed_quality.video.id = 120;
        changed_quality.video.quality = 120;
        assert_ne!(
            crate::api::plan_fingerprint(&changed_quality).unwrap(),
            plan.fingerprint
        );
        assert_eq!(
            core.enqueue(changed_quality).await.unwrap_err().code,
            ErrorCode::Conflict
        );
        let mut changed_audio = plan.clone();
        let mut audio = plan.video.clone();
        audio.audio = true;
        audio.id = 30280;
        audio.codec = "mp4a.40.2".into();
        audio.width = None;
        audio.height = None;
        changed_audio.audio = Some(audio);
        assert_ne!(
            crate::api::plan_fingerprint(&changed_audio).unwrap(),
            plan.fingerprint
        );
        assert_eq!(
            core.enqueue(changed_audio).await.unwrap_err().code,
            ErrorCode::Conflict
        );
        assert!(!owned.exists());
        assert_eq!(
            std::fs::read(output).unwrap(),
            b"previously committed output bytes"
        );
        core.shutdown(StopMode::Pause).await.unwrap();
    }
}

#[cfg(test)]
mod media_client_timeout_tests {
    use super::media_client;
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        task::JoinHandle,
    };

    const CLIENT_TIMEOUT: Duration = Duration::from_secs(1);
    const CONTINUOUS_BODY: &[u8] = b"0123456789abcdefghij";
    #[tokio::test]
    async fn continuous_body_longer_than_timeout_completes() {
        let (url, listener) = local_listener().await;
        let mut server = tokio::spawn(async move {
            continuous_server(listener).await;
        });
        let client = match media_client(CLIENT_TIMEOUT) {
            Ok(client) => client,
            Err(_) => {
                server.abort();
                let _ = server.await;
                panic!("anonymous media client setup failed");
            }
        };
        let outcome = observe_body(&client, &url, CONTINUOUS_BODY).await;
        let server_finished = finish_server(&mut server).await;

        eprintln!(
            "case=continuous stage={} is_timeout={} body_complete={} server_finished={server_finished}",
            outcome.stage, outcome.is_timeout, outcome.body_complete
        );
        assert!(server_finished, "continuous local server did not stop");
        assert_eq!(
            outcome.stage, "complete",
            "continuous body did not complete"
        );
        assert!(outcome.body_complete, "continuous body bytes differed");
    }

    #[tokio::test]
    async fn stalled_body_is_classified_as_a_body_timeout() {
        let (url, listener) = local_listener().await;
        let mut server = tokio::spawn(async move {
            stalled_server(listener).await;
        });
        let client = match media_client(CLIENT_TIMEOUT) {
            Ok(client) => client,
            Err(_) => {
                server.abort();
                let _ = server.await;
                panic!("anonymous media client setup failed");
            }
        };
        let outcome = observe_body(&client, &url, b"xy").await;
        let server_finished = finish_server(&mut server).await;

        eprintln!(
            "case=stalled stage={} is_timeout={} body_complete={} server_finished={server_finished}",
            outcome.stage, outcome.is_timeout, outcome.body_complete
        );
        assert!(server_finished, "stalled local server did not stop");
        assert_eq!(outcome.stage, "body", "stall did not occur in the body");
        assert!(
            outcome.is_timeout,
            "stalled body was not classified as timeout"
        );
        assert!(
            !outcome.body_complete,
            "stalled response body unexpectedly completed"
        );
    }

    struct SafeOutcome {
        stage: &'static str,
        is_timeout: bool,
        body_complete: bool,
    }

    async fn observe_body(client: &reqwest::Client, url: &str, expected: &[u8]) -> SafeOutcome {
        match client.get(url).send().await {
            Ok(response) => match response.bytes().await {
                Ok(body) => SafeOutcome {
                    stage: "complete",
                    is_timeout: false,
                    body_complete: body.as_ref() == expected,
                },
                Err(error) => SafeOutcome {
                    stage: "body",
                    is_timeout: error.is_timeout(),
                    body_complete: false,
                },
            },
            Err(error) => SafeOutcome {
                stage: "send",
                is_timeout: error.is_timeout(),
                body_complete: false,
            },
        }
    }

    async fn local_listener() -> (String, TcpListener) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback timeout fixture");
        let address = listener.local_addr().expect("read loopback address");
        (format!("http://{address}/"), listener)
    }

    async fn read_request_headers(socket: &mut TcpStream) -> std::io::Result<()> {
        let mut request = Vec::with_capacity(1024);
        let mut byte = [0u8; 1];
        while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
            if socket.read(&mut byte).await? == 0 {
                break;
            }
            request.push(byte[0]);
        }
        Ok(())
    }

    async fn continuous_server(listener: TcpListener) {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        if read_request_headers(&mut socket).await.is_err()
            || socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\n")
                .await
                .is_err()
        {
            return;
        }
        for byte in CONTINUOUS_BODY {
            if socket.write_all(&[*byte]).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn stalled_server(listener: TcpListener) {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        if read_request_headers(&mut socket).await.is_err()
            || socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nx")
                .await
                .is_err()
        {
            return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = socket.write_all(b"y").await;
    }

    async fn finish_server(server: &mut JoinHandle<()>) -> bool {
        match tokio::time::timeout(Duration::from_secs(4), &mut *server).await {
            Ok(_) => true,
            Err(_) => {
                server.abort();
                let _ = server.await;
                false
            }
        }
    }
}
