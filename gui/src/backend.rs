//! Bounded native-shell adapter. Core owns all network and durable job behavior.
//! The host retains this worker and joins it before dropping its Tokio runtime.
use bili_core::{auth::QrStatus, *};
use std::{
    collections::HashSet, future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration,
};
use tokio::{
    runtime::Handle,
    sync::{mpsc, watch},
    task::{JoinHandle, JoinSet},
};

#[derive(Clone)]
pub struct PrepareInput {
    pub input: String,
    pub parts: Option<Vec<PartRef>>,
    pub category: SearchCategory,
}
#[derive(Clone)]
pub struct PreparedItem {
    pub item: MediaItem,
    pub category: SearchCategory,
    pub selected: Vec<PartRef>,
    pub catalogs: Vec<(PartRef, Result<QualityCatalog>)>,
}
#[derive(Clone)]
pub struct AdmissionReceipt {
    pub id: JobId,
    pub plan: StreamPlan,
}
#[derive(Clone)]
pub struct AdmissionRequest {
    pub selection: DownloadSelection,
    pub context: DownloadContext,
}

pub enum Operation {
    Search(SearchQuery),
    Resolve(String),
    Enqueue {
        selections: Vec<AdmissionRequest>,
        output_dir: PathBuf,
    },
    Prepare {
        inputs: Vec<PrepareInput>,
        profile: Option<String>,
        refresh: bool,
    },
    Profiles,
    SelectProfile(String),
    ForgetProfile(String),
    OutputInfo(JobId),
    Remove {
        ids: Vec<JobId>,
        mode: RemoveMode,
    },
    RetargetProfile {
        id: JobId,
        profile: Option<String>,
    },
    Reveal {
        path: PathBuf,
        select: bool,
    },
    BrowseConditions(PgcSection),
    Browse(BrowseQuery),
    BeginVerification {
        request: VerificationRequest,
        profile: Option<String>,
    },
    CompleteVerification {
        challenge: VerificationChallenge,
        proof: VerificationProof,
    },
    Login {
        remember: bool,
        activate: bool,
    },
    RestoreSession,
    Logout,
    Anonymous,
    ValidateOutput(PathBuf),
}

pub enum Command {
    Run(Operation),
    Cancel,
    Control(JobId, JobAction),
    Cover(String),
    RetryInit,
}

pub enum Outcome {
    Search(CandidatePage),
    Detail(MediaItem),
    Enqueued {
        receipts: Vec<AdmissionReceipt>,
        cancelled: bool,
        error: Option<CoreError>,
    },
    Prepared {
        items: Vec<PreparedItem>,
        profile: Option<String>,
        refresh: bool,
    },
    OutputInfo(OutputInfo),
    Removed(RemovalReport),
    Retargeted(JobId),
    Revealed,
    BrowseConditions(BrowseConditions),
    Verification(VerificationChallenge),
    Verified,
    Auth(AuthSnapshot),
    ProfileAdded {
        auth: AuthSnapshot,
        profile_id: String,
    },
    Output(PathBuf),
}

pub enum Event {
    Ready,
    InitFailed(CoreError),
    Finished(Result<Outcome>),
    Login(AuthSnapshot),
    Controlled(JobId, Result<()>),
    Cover {
        url: String,
        image: Option<egui::ColorImage>,
    },
    /// Read-only decoration was preempted, rather than failing to load.
    CoverDeferred {
        url: String,
    },
    Stopped(Result<ShutdownReport>),
}

pub struct Bridge {
    commands: mpsc::Sender<Command>,
    covers: mpsc::Sender<String>,
    pub events: mpsc::Receiver<Event>,
    pub snapshots: watch::Receiver<Arc<QueueSnapshot>>,
    shutdown: watch::Sender<bool>,
}

const COVER_CONCURRENCY: usize = 2;
const COVER_QUEUE: usize = 16;

struct SessionIo {
    commands: mpsc::Receiver<Command>,
    covers: mpsc::Receiver<String>,
    events: mpsc::Sender<Event>,
    snapshots: watch::Sender<Arc<QueueSnapshot>>,
    stop: watch::Receiver<bool>,
    ctx: egui::Context,
}

pub struct Owner {
    worker: JoinHandle<()>,
    shutdown: watch::Sender<bool>,
}

impl Owner {
    /// Called by main only after the native event loop returns, never in paint.
    pub async fn finish(self) {
        self.shutdown.send_replace(true);
        if self.worker.await.is_err() {
            eprintln!("GUI backend worker failed; durable recovery may be required.");
        }
    }
}

impl Bridge {
    fn channel_pair(ctx: egui::Context) -> (Self, SessionIo) {
        let (commands, receiver) = mpsc::channel(16);
        let (covers, cover_requests) = mpsc::channel(COVER_QUEUE);
        let (events_tx, events) = mpsc::channel(32);
        let (snapshots_tx, snapshots) = watch::channel(Arc::new(QueueSnapshot::default()));
        let (shutdown, stop) = watch::channel(false);
        (
            Self {
                commands,
                covers,
                events,
                snapshots,
                shutdown,
            },
            SessionIo {
                commands: receiver,
                covers: cover_requests,
                events: events_tx,
                snapshots: snapshots_tx,
                stop,
                ctx,
            },
        )
    }
    #[cfg(test)]
    pub fn test_channel() -> (Self, mpsc::Sender<Event>, mpsc::Receiver<Command>) {
        let (bridge, io) = Self::channel_pair(egui::Context::default());
        (bridge, io.events, io.commands)
    }
    pub fn start(
        config: Result<CoreConfig>,
        runtime: &Handle,
        ctx: egui::Context,
    ) -> (Self, Owner) {
        let (bridge, io) = Self::channel_pair(ctx);
        let shutdown = bridge.shutdown.clone();
        let worker = runtime.spawn(serve(config, io));
        (bridge, Owner { worker, shutdown })
    }

    pub fn send(&self, command: Command) -> std::result::Result<(), &'static str> {
        if let Command::Cover(url) = command {
            // Decoration cannot fill the command lane or reserve unbounded
            // connection waiters. Rejected covers may be requested next frame.
            return self.covers.try_send(url).map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => "封面队列已满，稍后重试。",
                mpsc::error::TrySendError::Closed(_) => "后台已停止，请关闭并重新打开应用。",
            });
        }
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => "操作队列已满，请稍后再试。",
                mpsc::error::TrySendError::Closed(_) => "后台已停止，请关闭并重新打开应用。",
            })
    }

    pub fn shutdown(&self) {
        // A separate watch lane means a full command queue cannot prevent close.
        self.shutdown.send_replace(true);
    }
}

async fn emit(events: &mpsc::Sender<Event>, ctx: &egui::Context, event: Event) {
    if events.send(event).await.is_ok() {
        ctx.request_repaint();
    }
}

fn cancelled() -> CoreError {
    CoreError::new(ErrorCode::Cancelled, "GUI operation cancelled")
}

async fn wait_cancel(cancel: &mut watch::Receiver<bool>) {
    if *cancel.borrow() {
        return;
    }
    let _ = cancel.changed().await;
}

async fn initialize(config: Result<CoreConfig>) -> Result<Core> {
    let handle = Handle::current();
    tokio::task::spawn_blocking(move || Core::new(config?, handle))
        .await
        .map_err(|_| CoreError::new(ErrorCode::Corruption, "initialize GUI core worker"))?
}

async fn serve(config: Result<CoreConfig>, mut io: SessionIo) {
    // Initialization and retry never perform authentication or discovery calls.
    let core = loop {
        if *io.stop.borrow() {
            emit(
                &io.events,
                &io.ctx,
                Event::Stopped(Ok(ShutdownReport::default())),
            )
            .await;
            return;
        }
        match initialize(config.clone()).await {
            Ok(core) => break core,
            Err(error) => emit(&io.events, &io.ctx, Event::InitFailed(error)).await,
        }
        loop {
            tokio::select! {
                biased;
                _ = wait_cancel(&mut io.stop) => {
                    emit(&io.events, &io.ctx, Event::Stopped(Ok(ShutdownReport::default()))).await;
                    return;
                }
                command = io.commands.recv() => match command {
                    Some(Command::RetryInit) => break,
                    None => return,
                    _ => {}
                }
            }
        }
    };
    let jobs = core.jobs();
    let events = io.events.clone();
    let ctx = io.ctx.clone();
    let snapshots = io.snapshots.clone();
    let worker_error = serve_session(core.clone(), io, jobs.clone()).await;
    let report = core.shutdown(StopMode::Pause).await;
    let report = report.and_then(|report| worker_error.map_or(Ok(report), Err));
    snapshots.send_replace(jobs.borrow().clone());
    emit(&events, &ctx, Event::Stopped(report)).await;
}

type SessionFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// The private adapter keeps the real bridge scheduler testable without a core
/// instance, credentials, filesystem state or network service.
trait Session: Clone + Send + 'static {
    fn run(
        self,
        operation: Operation,
        cancel: watch::Receiver<bool>,
        events: mpsc::Sender<Event>,
        ctx: egui::Context,
    ) -> SessionFuture<Result<Outcome>>;
    fn control(self, id: JobId, action: JobAction) -> SessionFuture<(JobId, Result<()>)>;
    fn cover(self, url: String) -> SessionFuture<(String, Option<egui::ColorImage>)>;
}

impl Session for Core {
    fn run(
        self,
        operation: Operation,
        cancel: watch::Receiver<bool>,
        events: mpsc::Sender<Event>,
        ctx: egui::Context,
    ) -> SessionFuture<Result<Outcome>> {
        Box::pin(execute(self, operation, cancel, events, ctx))
    }
    fn control(self, id: JobId, action: JobAction) -> SessionFuture<(JobId, Result<()>)> {
        Box::pin(async move { (id, Core::control(&self, id, action).await) })
    }
    fn cover(self, url: String) -> SessionFuture<(String, Option<egui::ColorImage>)> {
        Box::pin(async move {
            let image = Core::cover(&self, CoverRequest { url: url.clone() })
                .await
                .ok()
                .and_then(decode_cover);
            (url, image)
        })
    }
}

async fn session_emit(io: &mut SessionIo, event: Event) -> bool {
    tokio::select! {
        biased;
        _ = wait_cancel(&mut io.stop) => false,
        result = io.events.send(event) => {
            if result.is_ok() { io.ctx.request_repaint(); }
            result.is_ok()
        }
    }
}

async fn serve_session<S: Session>(
    session: S,
    mut io: SessionIo,
    mut jobs: watch::Receiver<Arc<QueueSnapshot>>,
) -> Option<CoreError> {
    io.snapshots.send_replace(jobs.borrow_and_update().clone());
    if !session_emit(&mut io, Event::Ready).await {
        return None;
    }
    let mut foreground = JoinSet::new();
    let mut controls = JoinSet::new();
    let mut covers = JoinSet::new();
    let mut active_covers = HashSet::new();
    let mut cancellation: Option<watch::Sender<bool>> = None;
    let mut worker_error = None;
    'session: loop {
        tokio::select! {
            biased;
            _ = wait_cancel(&mut io.stop) => break,
            result = foreground.join_next(), if !foreground.is_empty() => {
                cancellation = None;
                let result = match result {
                    Some(Ok(result)) => result,
                    _ => Err(CoreError::new(ErrorCode::Corruption, "GUI operation worker failed")),
                };
                if !session_emit(&mut io, Event::Finished(result)).await { break; }
            }
            result = controls.join_next(), if !controls.is_empty() => {
                if let Some(Ok((id, result))) = result {
                    if !session_emit(&mut io, Event::Controlled(id, result)).await { break; }
                } else {
                    // Control tasks contain only a core call; a panic is fatal to
                    // this shell session rather than leaving buttons pending forever.
                    worker_error = Some(CoreError::new(ErrorCode::Corruption, "GUI control worker failed"));
                    break;
                }
            }
            command = io.commands.recv() => match command {
                Some(Command::Run(operation)) if foreground.is_empty() => {
                    // Release both running requests and semaphore waiters before
                    // foreground discovery, and discard the preceding page's
                    // queued decoration without poisoning its cache entries.
                    covers.abort_all();
                    while covers.join_next().await.is_some() {}
                    let mut deferred: Vec<_> = active_covers.drain().collect();
                    while let Ok(url) = io.covers.try_recv() { deferred.push(url); }
                    let (tx, rx) = watch::channel(false);
                    cancellation = Some(tx);
                    foreground.spawn(session.clone().run(operation, rx, io.events.clone(), io.ctx.clone()));
                    for url in deferred {
                        if !session_emit(&mut io, Event::CoverDeferred { url }).await { break 'session; }
                    }
                }
                Some(Command::Run(_)) => {
                    if !session_emit(&mut io, Event::Finished(Err(CoreError::new(ErrorCode::Busy, "GUI operation already running")))).await { break; }
                }
                Some(Command::Cancel) => {
                    if let Some(cancel) = &cancellation { cancel.send_replace(true); }
                }
                Some(Command::Control(id, action)) => {
                    if controls.len() < 16 {
                        controls.spawn(session.clone().control(id, action));
                    } else {
                        if !session_emit(&mut io, Event::Controlled(id, Err(CoreError::new(ErrorCode::Busy, "too many GUI controls")))).await { break; }
                    }
                }
                Some(Command::Cover(url)) => {
                    // Bridge::send routes covers into the bounded decoration lane.
                    if !session_emit(&mut io, Event::CoverDeferred { url }).await { break; }
                }
                Some(Command::RetryInit) => {}
                None => break,
            },
            result = covers.join_next(), if !covers.is_empty() => {
                if let Some(Ok((url, image))) = result {
                    active_covers.remove(&url);
                    if !session_emit(&mut io, Event::Cover { url, image }).await { break; }
                } else {
                    // A decoration panic must not leak pending cache entries or
                    // grow the active-name set across later pages.
                    covers.abort_all();
                    while covers.join_next().await.is_some() {}
                    for url in active_covers.drain() {
                        if !session_emit(&mut io, Event::CoverDeferred { url }).await { break 'session; }
                    }
                }
            }
            Some(url) = io.covers.recv(), if foreground.is_empty() && covers.len() < COVER_CONCURRENCY => {
                if active_covers.insert(url.clone()) {
                    covers.spawn(session.clone().cover(url));
                }
            }
            changed = jobs.changed() => {
                if changed.is_err() {
                    worker_error = Some(CoreError::new(ErrorCode::Shutdown, "core snapshot stream closed"));
                    break;
                }
                io.snapshots.send_replace(jobs.borrow_and_update().clone());
                io.ctx.request_repaint();
            }
        }
    }
    io.commands.close();
    io.covers.close();
    if let Some(cancel) = cancellation {
        cancel.send_replace(true);
    }
    // Release read-only HTTP resources before waiting for durable acknowledgements.
    covers.abort_all();
    while covers.join_next().await.is_some() {}
    // Do not abort state-changing futures: wait for enqueue/control acks, then
    // let core pause, checkpoint and join its owned transfer/remux workers.
    while foreground.join_next().await.is_some() {}
    while controls.join_next().await.is_some() {}
    worker_error
}

async fn execute(
    core: Core,
    operation: Operation,
    mut cancel: watch::Receiver<bool>,
    events: mpsc::Sender<Event>,
    ctx: egui::Context,
) -> Result<Outcome> {
    match operation {
        Operation::Search(query) => tokio::select! {
            biased;
            _ = wait_cancel(&mut cancel) => Err(cancelled()),
            result = core.search(query) => result.map(Outcome::Search),
        },
        Operation::Resolve(input) => tokio::select! {
            biased;
            _ = wait_cancel(&mut cancel) => Err(cancelled()),
            result = core.resolve(input.into()) => result.map(Outcome::Detail),
        },
        Operation::Prepare {
            inputs,
            profile,
            refresh,
        } => {
            let mut prepared = Vec::new();
            for input in inputs {
                let item = tokio::select! {
                    biased;
                    _ = wait_cancel(&mut cancel) => return Err(cancelled()),
                    result = core.resolve_for(input.input.into(), profile.clone()) => result?,
                };
                let selected = input
                    .parts
                    .unwrap_or_else(|| item.parts.iter().map(|p| p.reference.clone()).collect());
                let mut catalogs = Vec::new();
                for part in &item.parts {
                    let result = tokio::select! {
                        biased;
                        _ = wait_cancel(&mut cancel) => return Err(cancelled()),
                        result = core.quality_catalog_for(part.reference.clone(), profile.clone()) => result,
                    };
                    catalogs.push((part.reference.clone(), result));
                }
                prepared.push(PreparedItem {
                    item,
                    category: input.category,
                    selected,
                    catalogs,
                });
            }
            Ok(Outcome::Prepared {
                items: prepared,
                profile,
                refresh,
            })
        }
        Operation::Profiles => core.auth(AuthAction::ListProfiles).await.map(Outcome::Auth),
        Operation::SelectProfile(id) => core
            .auth(AuthAction::SelectProfile { id })
            .await
            .map(Outcome::Auth),
        Operation::ForgetProfile(id) => core
            .auth(AuthAction::ForgetProfile { id })
            .await
            .map(Outcome::Auth),
        Operation::OutputInfo(id) => core.output_info(id).await.map(Outcome::OutputInfo),
        Operation::Remove { ids, mode } => core.remove_jobs(ids, mode).await.map(Outcome::Removed),
        Operation::RetargetProfile { id, profile } => core
            .retarget_profile(id, profile)
            .await
            .map(|_| Outcome::Retargeted(id)),
        Operation::Reveal { path, select } => {
            tokio::task::spawn_blocking(move || reveal(path, select))
                .await
                .map_err(|_| CoreError::new(ErrorCode::Filesystem, "open output location"))?
                .map(|_| Outcome::Revealed)
        }
        Operation::BrowseConditions(section) => {
            cancellable(&mut cancel, core.browse_conditions(section))
                .await
                .map(Outcome::BrowseConditions)
        }
        Operation::Browse(query) => tokio::select! {
            biased;
            _ = wait_cancel(&mut cancel) => Err(cancelled()),
            result = core.browse(query) => result.map(Outcome::Search),
        },
        Operation::BeginVerification { request, profile } => core
            .begin_verification(request, profile)
            .await
            .map(Outcome::Verification),
        Operation::CompleteVerification { challenge, proof } => core
            .complete_verification(challenge, proof)
            .await
            .map(|_| Outcome::Verified),
        Operation::Enqueue {
            selections,
            output_dir,
        } => {
            let mut receipts = Vec::new();
            // Preflight every pending selection before admitting any item.
            // This makes an unsupported strict quality a blocking batch error.
            let mut plans = Vec::new();
            for request in selections {
                let result = tokio::select! {
                    biased;
                    _ = wait_cancel(&mut cancel) => {
                        return Ok(Outcome::Enqueued { receipts, cancelled: true, error: None });
                    }
                    result = core.plan_in_directory(request.selection, request.context, output_dir.clone()) => result,
                };
                let plan = match result {
                    Ok(plan) => plan,
                    Err(error) => {
                        return Ok(Outcome::Enqueued {
                            receipts,
                            cancelled: false,
                            error: Some(error),
                        })
                    }
                };
                plans.push(plan);
            }
            for plan in plans {
                if *cancel.borrow() {
                    return Ok(Outcome::Enqueued {
                        receipts,
                        cancelled: true,
                        error: None,
                    });
                }
                // Once sent, await the durable acknowledgement even on cancel.
                match core.enqueue(plan.clone()).await {
                    Ok(id) => receipts.push(AdmissionReceipt { id, plan }),
                    Err(error) => {
                        return Ok(Outcome::Enqueued {
                            receipts,
                            cancelled: false,
                            error: Some(error),
                        })
                    }
                }
            }
            Ok(Outcome::Enqueued {
                receipts,
                cancelled: false,
                error: None,
            })
        }
        Operation::Login { remember, activate } => {
            let snapshot = login(&core, remember, activate, &mut cancel, &events, &ctx).await?;
            if activate {
                Ok(Outcome::Auth(snapshot))
            } else {
                let profile_id = snapshot.login_profile.ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::Corruption,
                        "completed QR profile receipt missing",
                    )
                })?;
                let auth = core.auth(AuthAction::ListProfiles).await?;
                Ok(Outcome::ProfileAdded { auth, profile_id })
            }
        }
        Operation::RestoreSession => core
            .auth(AuthAction::LoadStored { required: false })
            .await
            .map(Outcome::Auth),
        Operation::Logout => core.auth(AuthAction::Logout).await.map(Outcome::Auth),
        Operation::Anonymous => core.auth(AuthAction::Anonymous).await.map(Outcome::Auth),
        Operation::ValidateOutput(path) => {
            tokio::task::spawn_blocking(move || validate_output(path))
                .await
                .map_err(|_| CoreError::new(ErrorCode::Filesystem, "check GUI output directory"))?
                .map(Outcome::Output)
        }
    }
}

async fn cancellable<T>(
    cancel: &mut watch::Receiver<bool>,
    request: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = wait_cancel(cancel) => Err(cancelled()),
        result = request => result,
    }
}

async fn login(
    core: &Core,
    remember: bool,
    activate: bool,
    cancel: &mut watch::Receiver<bool>,
    events: &mpsc::Sender<Event>,
    ctx: &egui::Context,
) -> Result<AuthSnapshot> {
    login_with_auth(remember, activate, cancel, events, ctx, |action| {
        core.auth(action)
    })
    .await
}

async fn login_with_auth<F, Response>(
    remember: bool,
    activate: bool,
    cancel: &mut watch::Receiver<bool>,
    events: &mpsc::Sender<Event>,
    ctx: &egui::Context,
    auth: F,
) -> Result<AuthSnapshot>
where
    F: Fn(AuthAction) -> Response,
    Response: std::future::Future<Output = Result<AuthSnapshot>>,
{
    let result = async {
        let start = auth(AuthAction::StartQr { remember, activate }).await?;
        let expires = start
            .qr
            .as_ref()
            .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "QR challenge absent"))?
            .expires_in_secs;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(expires);
        emit(events, ctx, Event::Login(start)).await;
        loop {
            tokio::select! {
                biased;
                _ = wait_cancel(cancel) => return Err(cancelled()),
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(CoreError::new(ErrorCode::LoginRequired, "QR login expired"));
                }
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
            // Once core has installed a new profile, report its acknowledgement
            // even if cancellation raced that commit. A previously logged-in
            // account alone is not completion of an add-account QR challenge.
            let snapshot = auth(AuthAction::PollQr).await?;
            if completed_qr(&snapshot) {
                return Ok(snapshot);
            }
            if *cancel.borrow() {
                return Err(cancelled());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(CoreError::new(ErrorCode::LoginRequired, "QR login expired"));
            }
            if matches!(
                snapshot.qr_status,
                Some(QrStatus::Expired | QrStatus::Cancelled)
            ) {
                return Err(CoreError::new(ErrorCode::LoginRequired, "QR login expired"));
            }
            emit(events, ctx, Event::Login(snapshot)).await;
        }
    }
    .await;
    if result.is_err() {
        let _ = auth(AuthAction::CancelQr).await;
    }
    result
}

fn completed_qr(snapshot: &AuthSnapshot) -> bool {
    snapshot.logged_in
        && snapshot.qr_status == Some(QrStatus::Complete)
        && snapshot.login_profile.is_some()
}

fn decode_cover(bytes: CoverBytes) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(&bytes.bytes).ok()?;
    let thumb = image.thumbnail(480, 480).into_rgba8();
    let (width, height) = thumb.dimensions();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        thumb.as_raw(),
    ))
}

/// Core invokes this host adapter only with its verified exact owned allowlist.
#[cfg(windows)]
pub(crate) fn recycle_files(paths: &[PathBuf]) -> Result<()> {
    trash::delete_all(paths)
        .map_err(|_| CoreError::new(ErrorCode::Filesystem, "move owned output to recycle bin"))
}

fn validate_output(path: PathBuf) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(CoreError::new(
            ErrorCode::InvalidInput,
            "output directory must be absolute",
        ));
    }
    let mut existing = path.as_path();
    loop {
        match std::fs::metadata(existing) {
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.permissions().readonly() {
                    return Err(CoreError::new(
                        ErrorCode::Filesystem,
                        "output directory is not writable",
                    ));
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                existing = existing.parent().ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::Filesystem,
                        "output directory has no accessible parent",
                    )
                })?;
            }
            Err(error) => return Err(CoreError::io(error)),
        }
    }
}

fn reveal(path: PathBuf, select: bool) -> Result<()> {
    if !path.is_absolute() || !path.exists() {
        return Err(CoreError::new(
            ErrorCode::Filesystem,
            "output location unavailable",
        ));
    }
    #[cfg(windows)]
    {
        let mut command = std::process::Command::new("explorer.exe");
        if select {
            command.arg("/select,");
        }
        command.arg(&path).spawn().map_err(CoreError::io)?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let directory = if select {
            path.parent()
                .ok_or_else(|| CoreError::new(ErrorCode::Filesystem, "output parent unavailable"))?
        } else {
            path.as_path()
        };
        std::process::Command::new("xdg-open")
            .arg(directory)
            .spawn()
            .map_err(CoreError::io)?;
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = select;
        Err(CoreError::new(
            ErrorCode::NativeCapability,
            "file location unavailable on this platform",
        ))
    }
}

pub fn error_message(error: &CoreError) -> String {
    if error.http_status == Some(429) {
        return "请求过于频繁（HTTP 429），已暂停向站点发送 API 请求。请稍后再试。".into();
    }
    if error.http_status == Some(412) {
        return "站点暂时拒绝了此请求（HTTP 412），请稍后重试。".into();
    }
    let specific=match (error.code,error.operation.as_str()){
        (ErrorCode::LoginRequired,"selected credential profile unavailable")=>Some("此任务原先使用的账号已不可用。请选择当前已登录的账号后重试。"),
        (ErrorCode::RepresentationChanged,"selected representation verification")=>Some("已下载文件未通过所选规格校验，恢复数据已保留。可重试校验；若规格仍不符，请重新选择。"),
        (ErrorCode::Conflict,"completed output ownership or content changed"|"refusing unowned or modified output removal"|"refusing unowned component removal"|"owned component content changed")=>Some("文件已被修改，或不属于这条下载记录，已保留文件。可以仅移除记录。"),
        (ErrorCode::Conflict,"output already exists"|"output already exists without matching completion"|"manual output already exists"|"unowned component output exists")=>Some("保存位置已有不匹配的文件，已保留原文件。请选择其他目录后重试。"),
        (ErrorCode::Conflict,"completed job retains its original admission settings; choose another output directory")=>Some("已有完成记录会保留原来的账号与设置。若需要新设置，请选择其他保存目录。"),
        (ErrorCode::Conflict,"batch rate differs from admitted settings")=>Some("此批次已采用其他限速设置，请重新配置为新批次。"),
        (ErrorCode::NativeCapability,"host recycle bin adapter unavailable")=>Some("当前平台无法使用回收站，未删除文件。可以仅移除记录。"),
        (ErrorCode::Filesystem,"output location unavailable")=>Some("文件或目录已不存在，无法打开所在位置。"),
        (ErrorCode::Busy,"state directory already has a writer")=>Some("下载记录正在被另一个进程使用。请先退出使用此状态目录的 CLI 或 GUI。"),
        _=>None,
    };
    if let Some(message) = specific {
        return message.into();
    }
    let help = match error.code {
        ErrorCode::InvalidInput => "输入无效，请检查编号、链接或绝对路径。",
        ErrorCode::VerificationRequired => {
            "站点要求安全验证。完成验证后可重试；没有可用验证入口时，请先重试获取新验证信息。"
        }
        ErrorCode::LoginRequired => "需要登录，或二维码已失效。请重新扫码。",
        ErrorCode::EntitlementRequired => "当前账号没有此内容的观看权限。",
        ErrorCode::RegionRestricted => "内容在当前地区不可用。",
        ErrorCode::Unavailable => "内容暂不可用或已下架。",
        ErrorCode::RepresentationChanged => "媒体规格已变化，请重新解析后添加。",
        ErrorCode::Transport | ErrorCode::RetryExhausted => "网络请求未成功，请检查连接后重试。",
        ErrorCode::Filesystem => "无法访问文件。请检查目录、权限和剩余空间。",
        ErrorCode::Conflict => {
            "此操作与当前任务或文件状态冲突，原记录与文件已保留。请查看错误信息后调整操作。"
        }
        ErrorCode::Busy => "当前操作尚未结束，请等待或先暂停相关任务。",
        ErrorCode::NativeCapability | ErrorCode::Remux => {
            "原生媒体处理不可用，请检查配套 FFmpeg 共享库。"
        }
        ErrorCode::Cancelled => "操作已取消。",
        ErrorCode::Corruption => "本地状态或后台任务异常，请保留状态目录后排查。",
        ErrorCode::ReadOnly => "当前状态目录只读，无法修改任务。",
        ErrorCode::Shutdown => "后台正在关闭或已停止。",
        ErrorCode::CredentialStore => {
            "系统凭据库不可用；可改用「仅本次登录」，或检查系统凭据服务。"
        }
    };
    // Never render remote error bodies, URLs, cookies, or QR tokens.
    help.to_owned()
}

pub fn error_code(error: &CoreError) -> String {
    let mut code = format!("错误码 {:?}", error.code);
    if let Some(status) = error
        .http_status
        .filter(|status| (100..=599).contains(status))
    {
        code.push_str(&format!(" · HTTP {status}"));
    }
    code
}

/// Only exact core-owned labels are mapped; never echo operation text, URLs,
/// remote bodies, cookies, or an inferred number of retry attempts.
pub fn error_hint(error: &CoreError) -> Option<String> {
    if !matches!(error.code, ErrorCode::Transport | ErrorCode::RetryExhausted) {
        return None;
    }
    let stage = match error.operation.as_str() {
        "request Bilibili API timed out" => Some("请求超时"),
        "connect to Bilibili API" => Some("连接失败"),
        "request Bilibili API" => Some("请求未成功"),
        "read Bilibili API response" => Some("读取响应失败"),
        "read Bilibili API response timed out" => Some("读取响应超时"),
        _ => None,
    };
    let mut details = Vec::new();
    if let Some(stage) = stage {
        details.push(stage.to_owned());
    }
    if let Some(status) = error
        .http_status
        .filter(|status| (100..=599).contains(status))
    {
        details.push(format!("HTTP {status}"));
    }
    if error.code == ErrorCode::RetryExhausted {
        details.push("自动重试已结束，可手动重试。".into());
    }
    (!details.is_empty()).then(|| details.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::Semaphore;

    #[derive(Clone)]
    struct FixtureSession {
        started: mpsc::Sender<&'static str>,
        release: Arc<Semaphore>,
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        admitted: Arc<AtomicBool>,
        controlled: Arc<AtomicBool>,
    }
    struct ActiveCover(Arc<AtomicUsize>);
    impl Drop for ActiveCover {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl Session for FixtureSession {
        fn run(
            self,
            operation: Operation,
            mut cancel: watch::Receiver<bool>,
            _: mpsc::Sender<Event>,
            _: egui::Context,
        ) -> SessionFuture<Result<Outcome>> {
            Box::pin(async move {
                self.started.send("foreground").await.unwrap();
                if matches!(operation, Operation::Enqueue { .. }) {
                    // Admission deliberately ignores cancellation once sent;
                    // the real scheduler must join its acknowledgement.
                    self.release.acquire().await.unwrap().forget();
                    self.admitted.store(true, Ordering::SeqCst);
                    return Ok(Outcome::Enqueued {
                        receipts: vec![],
                        cancelled: false,
                        error: None,
                    });
                }
                cancellable(&mut cancel, async move {
                    self.release.acquire().await.unwrap().forget();
                    match operation {
                        Operation::BrowseConditions(section) => {
                            Ok(Outcome::BrowseConditions(BrowseConditions {
                                section,
                                genres: vec![],
                            }))
                        }
                        _ => Ok(Outcome::Search(CandidatePage { pages: vec![] })),
                    }
                })
                .await
            })
        }
        fn control(self, id: JobId, _: JobAction) -> SessionFuture<(JobId, Result<()>)> {
            Box::pin(async move {
                self.started.send("control").await.unwrap();
                self.release.acquire().await.unwrap().forget();
                self.controlled.store(true, Ordering::SeqCst);
                (id, Ok(()))
            })
        }
        fn cover(self, url: String) -> SessionFuture<(String, Option<egui::ColorImage>)> {
            Box::pin(async move {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(active, Ordering::SeqCst);
                let _owned = ActiveCover(self.active.clone());
                self.started.send("cover").await.unwrap();
                std::future::pending::<()>().await;
                (url, None)
            })
        }
    }
    fn fixture() -> (FixtureSession, mpsc::Receiver<&'static str>) {
        let (started, receiver) = mpsc::channel(32);
        (
            FixtureSession {
                started,
                release: Arc::new(Semaphore::new(0)),
                active: Arc::new(AtomicUsize::new(0)),
                peak: Arc::new(AtomicUsize::new(0)),
                admitted: Arc::new(AtomicBool::new(false)),
                controlled: Arc::new(AtomicBool::new(false)),
            },
            receiver,
        )
    }
    async fn next_event(bridge: &mut Bridge) -> Event {
        tokio::time::timeout(Duration::from_secs(2), bridge.events.recv())
            .await
            .expect("bridge event deadline")
            .expect("bridge event channel")
    }
    async fn started(receiver: &mut mpsc::Receiver<&'static str>) -> &'static str {
        tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("operation start deadline")
            .unwrap()
    }
    fn query() -> SearchQuery {
        SearchQuery {
            keyword: "offline".into(),
            category: SearchCategory::Video,
            page: 1,
            sort: SearchSort::Relevance,
            filter: Default::default(),
        }
    }

    #[tokio::test]
    async fn bridge_reports_only_the_actual_terminal_result_of_a_delayed_operation() {
        let (session, mut requests) = fixture();
        let (mut bridge, io) = Bridge::channel_pair(egui::Context::default());
        let (_jobs, jobs) = watch::channel(Arc::new(QueueSnapshot::default()));
        let worker = tokio::spawn(serve_session(session.clone(), io, jobs));
        assert!(matches!(next_event(&mut bridge).await, Event::Ready));
        bridge
            .send(Command::Run(Operation::Search(query())))
            .unwrap();
        assert_eq!(started(&mut requests).await, "foreground");
        assert!(
            matches!(
                bridge.events.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ),
            "waiting is not a failed terminal result"
        );
        session.release.add_permits(1);
        assert!(matches!(
            next_event(&mut bridge).await,
            Event::Finished(Ok(Outcome::Search(_)))
        ));
        bridge.shutdown();
        assert!(tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn cover_flood_is_bounded_and_preempted_before_foreground_work() {
        let (session, mut requests) = fixture();
        let (mut bridge, io) = Bridge::channel_pair(egui::Context::default());
        let (_jobs, jobs) = watch::channel(Arc::new(QueueSnapshot::default()));
        let worker = tokio::spawn(serve_session(session.clone(), io, jobs));
        assert!(matches!(next_event(&mut bridge).await, Event::Ready));
        for i in 0..COVER_CONCURRENCY {
            bridge.send(Command::Cover(format!("active:{i}"))).unwrap();
        }
        for _ in 0..COVER_CONCURRENCY {
            assert_eq!(started(&mut requests).await, "cover");
        }
        let accepted = (0..100)
            .filter(|i| bridge.send(Command::Cover(format!("queued:{i}"))).is_ok())
            .count();
        assert_eq!(accepted, COVER_QUEUE);
        bridge
            .send(Command::Run(Operation::Search(query())))
            .expect("covers cannot fill foreground command queue");
        assert_eq!(started(&mut requests).await, "foreground");
        assert_eq!(
            session.active.load(Ordering::SeqCst),
            0,
            "cover HTTP futures must be dropped before discovery starts"
        );
        assert_eq!(session.peak.load(Ordering::SeqCst), COVER_CONCURRENCY);
        let mut deferred = HashSet::new();
        for _ in 0..COVER_QUEUE + COVER_CONCURRENCY {
            let Event::CoverDeferred { url } = next_event(&mut bridge).await else {
                panic!("preemption must release pending cache entries without a failure");
            };
            deferred.insert(url);
        }
        assert_eq!(deferred.len(), COVER_QUEUE + COVER_CONCURRENCY);
        session.release.add_permits(1);
        assert!(matches!(
            next_event(&mut bridge).await,
            Event::Finished(Ok(Outcome::Search(_)))
        ));
        bridge.send(Command::Cover("active:0".into())).unwrap();
        assert_eq!(
            started(&mut requests).await,
            "cover",
            "preempted URL remains requestable"
        );
        bridge.shutdown();
        assert!(tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .is_none());
        assert_eq!(
            session.active.load(Ordering::SeqCst),
            0,
            "close joins aborted cover work"
        );
    }

    #[tokio::test]
    async fn browse_conditions_can_be_cancelled_while_its_request_is_pending() {
        let (session, mut requests) = fixture();
        let (mut bridge, io) = Bridge::channel_pair(egui::Context::default());
        let (_jobs, jobs) = watch::channel(Arc::new(QueueSnapshot::default()));
        let worker = tokio::spawn(serve_session(session, io, jobs));
        assert!(matches!(next_event(&mut bridge).await, Event::Ready));
        bridge
            .send(Command::Run(Operation::BrowseConditions(PgcSection::Anime)))
            .unwrap();
        assert_eq!(started(&mut requests).await, "foreground");
        bridge.send(Command::Cancel).unwrap();
        assert!(
            matches!(next_event(&mut bridge).await, Event::Finished(Err(error)) if error.code == ErrorCode::Cancelled)
        );
        bridge.shutdown();
        assert!(tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn close_joins_admission_and_control_acknowledgements_without_aborting_them() {
        let (session, mut requests) = fixture();
        let (mut bridge, io) = Bridge::channel_pair(egui::Context::default());
        let (_jobs, jobs) = watch::channel(Arc::new(QueueSnapshot::default()));
        let worker = tokio::spawn(serve_session(session.clone(), io, jobs));
        assert!(matches!(next_event(&mut bridge).await, Event::Ready));
        bridge
            .send(Command::Run(Operation::Enqueue {
                selections: vec![],
                output_dir: PathBuf::from("synthetic"),
            }))
            .unwrap();
        bridge
            .send(Command::Control(JobId::new(), JobAction::Pause))
            .unwrap();
        let mut seen = [started(&mut requests).await, started(&mut requests).await];
        seen.sort_unstable();
        assert_eq!(seen, ["control", "foreground"]);
        bridge.shutdown();
        tokio::task::yield_now().await;
        assert!(!worker.is_finished());
        session.release.add_permits(2);
        assert!(tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .is_none());
        assert!(session.admitted.load(Ordering::SeqCst));
        assert!(session.controlled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn close_interrupts_event_backpressure_without_requiring_the_paint_loop() {
        let (session, _) = fixture();
        let (bridge, io) = Bridge::channel_pair(egui::Context::default());
        for _ in 0..32 {
            assert!(io.events.try_send(Event::Ready).is_ok());
        }
        let (_jobs, jobs) = watch::channel(Arc::new(QueueSnapshot::default()));
        let worker = tokio::spawn(serve_session(session, io, jobs));
        bridge.shutdown();
        assert!(tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap()
            .is_none());
    }
    #[test]
    fn adding_an_account_requires_the_new_qr_commit_not_an_existing_logged_in_session() {
        let mut snapshot = AuthSnapshot {
            logged_in: true,
            account: None,
            notice: None,
            qr: None,
            qr_status: Some(QrStatus::WaitingForScan),
            profiles: vec![],
            selected_profile: Some("existing-session".into()),
            login_profile: None,
        };
        assert!(!completed_qr(&snapshot));
        snapshot.qr_status = Some(QrStatus::Complete);
        assert!(!completed_qr(&snapshot));
        snapshot.login_profile = Some("new-profile".into());
        assert!(completed_qr(&snapshot));
        snapshot.logged_in = false;
        assert!(!completed_qr(&snapshot));
    }

    #[tokio::test]
    async fn cancelling_new_qr_keeps_an_existing_account_and_only_cancels_the_challenge() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        };
        for activate in [false, true] {
            let old = AuthSnapshot {
                logged_in: true,
                account: None,
                notice: None,
                qr: None,
                qr_status: None,
                profiles: vec![],
                selected_profile: Some("existing-profile".into()),
                login_profile: None,
            };
            let calls = Arc::new(Mutex::new(Vec::new()));
            let selected = Arc::new(AtomicBool::new(true));
            let seen = calls.clone();
            let current = selected.clone();
            let (_cancel, mut cancelled) = watch::channel(true);
            let (events, _receiver) = mpsc::channel(4);
            let result = login_with_auth(
                false,
                activate,
                &mut cancelled,
                &events,
                &egui::Context::default(),
                move |action| {
                    let snapshot = match action {
                        AuthAction::StartQr {
                            activate: requested,
                            ..
                        } => {
                            assert_eq!(requested, activate);
                            seen.lock().unwrap().push("start");
                            let mut pending = old.clone();
                            pending.logged_in = false;
                            pending.qr = Some(bili_core::auth::QrChallenge {
                                login_url: "https://passport.bilibili.com/test-synthetic-qr".into(),
                                expires_in_secs: 60,
                            });
                            pending.qr_status = Some(QrStatus::WaitingForScan);
                            Ok(pending)
                        }
                        AuthAction::CancelQr => {
                            seen.lock().unwrap().push("cancel");
                            Ok(old.clone())
                        }
                        AuthAction::Anonymous => {
                            current.store(false, Ordering::SeqCst);
                            seen.lock().unwrap().push("anonymous");
                            Ok(old.clone())
                        }
                        _ => Err(CoreError::new(
                            ErrorCode::InvalidInput,
                            "unexpected synthetic auth action",
                        )),
                    };
                    std::future::ready(snapshot)
                },
            )
            .await;
            assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled);
            assert!(selected.load(Ordering::SeqCst));
            assert_eq!(*calls.lock().unwrap(), vec!["start", "cancel"]);
        }
    }

    #[test]
    fn output_requires_absolute_path() {
        assert!(validate_output(PathBuf::from("relative/downloads")).is_err());
    }

    #[test]
    fn error_code_is_separate_from_actionable_message() {
        let error = CoreError::new(ErrorCode::Transport, "request");
        assert!(!error_message(&error).contains("Transport"));
        assert!(error_code(&error).contains("Transport"));
    }

    #[test]
    fn error_presentation_never_uses_untrusted_operation_text() {
        let mut error = CoreError::new(ErrorCode::Transport, "request");
        error.operation = "secret-cookie-and-url".into();
        assert!(!error_message(&error).contains("secret-cookie"));
        assert!(error_hint(&error).is_none());
        error.http_status = Some(503);
        assert_eq!(error_hint(&error).as_deref(), Some("HTTP 503"));
        assert!(!error_code(&error).contains("secret-cookie"));
    }

    #[test]
    fn transport_diagnostics_show_only_allowlisted_stage_and_actual_numeric_status() {
        let mut error =
            CoreError::new(ErrorCode::Transport, "read Bilibili API response timed out");
        error.http_status = Some(502);
        assert_eq!(
            error_hint(&error).as_deref(),
            Some("读取响应超时 · HTTP 502")
        );
        assert_eq!(error_code(&error), "错误码 Transport · HTTP 502");
        error.code = ErrorCode::RetryExhausted;
        assert_eq!(
            error_hint(&error).as_deref(),
            Some("读取响应超时 · HTTP 502 · 自动重试已结束，可手动重试。")
        );
        error.http_status = Some(999);
        assert!(!error_code(&error).contains("999"));
        assert!(!error_hint(&error).unwrap().contains("999"));
        error.code = ErrorCode::VerificationRequired;
        assert!(
            error_hint(&error).is_none(),
            "access challenges never get a transport diagnosis"
        );
    }
}
