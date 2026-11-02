//! Bounded native-shell adapter. Core owns all network and durable job behavior.
//! The host retains this worker and joins it before dropping its Tokio runtime.
use bili_core::{auth::QrStatus, *};
use std::{path::PathBuf, sync::Arc, time::Duration};
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
    Stopped(Result<ShutdownReport>),
}

pub struct Bridge {
    commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    pub snapshots: watch::Receiver<Arc<QueueSnapshot>>,
    shutdown: watch::Sender<bool>,
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
    #[cfg(test)]
    pub fn test_channel() -> (Self, mpsc::Sender<Event>, mpsc::Receiver<Command>) {
        let (commands, receiver) = mpsc::channel(16);
        let (events_tx, events) = mpsc::channel(32);
        let (_, snapshots) = watch::channel(Arc::new(QueueSnapshot::default()));
        let (shutdown, _) = watch::channel(false);
        (
            Self {
                commands,
                events,
                snapshots,
                shutdown,
            },
            events_tx,
            receiver,
        )
    }
    pub fn start(
        config: Result<CoreConfig>,
        runtime: &Handle,
        ctx: egui::Context,
    ) -> (Self, Owner) {
        let (commands, receiver) = mpsc::channel(16);
        let (events_tx, events) = mpsc::channel(32);
        let (snapshots_tx, snapshots) = watch::channel(Arc::new(QueueSnapshot::default()));
        let (shutdown, stop) = watch::channel(false);
        let worker = runtime.spawn(serve(config, receiver, events_tx, snapshots_tx, stop, ctx));
        (
            Self {
                commands,
                events,
                snapshots,
                shutdown: shutdown.clone(),
            },
            Owner { worker, shutdown },
        )
    }

    pub fn send(&self, command: Command) -> std::result::Result<(), &'static str> {
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

async fn serve(
    config: Result<CoreConfig>,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
    snapshots: watch::Sender<Arc<QueueSnapshot>>,
    mut stop: watch::Receiver<bool>,
    ctx: egui::Context,
) {
    // Initialization and retry never perform authentication or discovery calls.
    let core = loop {
        if *stop.borrow() {
            emit(&events, &ctx, Event::Stopped(Ok(ShutdownReport::default()))).await;
            return;
        }
        match initialize(config.clone()).await {
            Ok(core) => break core,
            Err(error) => emit(&events, &ctx, Event::InitFailed(error)).await,
        }
        loop {
            tokio::select! {
                biased;
                _ = wait_cancel(&mut stop) => {
                    emit(&events, &ctx, Event::Stopped(Ok(ShutdownReport::default()))).await;
                    return;
                }
                command = commands.recv() => match command {
                    Some(Command::RetryInit) => break,
                    None => return,
                    _ => {}
                }
            }
        }
    };
    let mut jobs = core.jobs();
    snapshots.send_replace(jobs.borrow_and_update().clone());
    emit(&events, &ctx, Event::Ready).await;
    let mut foreground = JoinSet::new();
    let mut controls = JoinSet::new();
    let mut covers = JoinSet::new();
    let mut cancellation: Option<watch::Sender<bool>> = None;
    let mut worker_error = None;
    loop {
        tokio::select! {
            biased;
            _ = wait_cancel(&mut stop) => break,
            result = foreground.join_next(), if !foreground.is_empty() => {
                cancellation = None;
                let result = match result {
                    Some(Ok(result)) => result,
                    _ => Err(CoreError::new(ErrorCode::Corruption, "GUI operation worker failed")),
                };
                emit(&events, &ctx, Event::Finished(result)).await;
            }
            result = controls.join_next(), if !controls.is_empty() => {
                if let Some(Ok((id, result))) = result {
                    emit(&events, &ctx, Event::Controlled(id, result)).await;
                } else {
                    // Control tasks contain only a core call; a panic is fatal to
                    // this shell session rather than leaving buttons pending forever.
                    worker_error = Some(CoreError::new(ErrorCode::Corruption, "GUI control worker failed"));
                    break;
                }
            }
            result = covers.join_next(), if !covers.is_empty() => {
                if let Some(Ok((url, image))) = result {
                    emit(&events, &ctx, Event::Cover { url, image }).await;
                }
            }
            command = commands.recv() => match command {
                Some(Command::Run(operation)) if foreground.is_empty() => {
                    let (tx, rx) = watch::channel(false);
                    cancellation = Some(tx);
                    foreground.spawn(execute(core.clone(), operation, rx, events.clone(), ctx.clone()));
                }
                Some(Command::Run(_)) => {
                    emit(&events, &ctx, Event::Finished(Err(CoreError::new(ErrorCode::Busy, "GUI operation already running")))).await;
                }
                Some(Command::Cancel) => {
                    if let Some(cancel) = &cancellation { cancel.send_replace(true); }
                }
                Some(Command::Control(id, action)) => {
                    if controls.len() < 16 {
                        let core = core.clone();
                        controls.spawn(async move { (id, core.control(id, action).await) });
                    } else {
                        emit(&events, &ctx, Event::Controlled(id, Err(CoreError::new(ErrorCode::Busy, "too many GUI controls")))).await;
                    }
                }
                Some(Command::Cover(url)) => {
                    // Covers are read-only decoration; keep them out of the
                    // single-slot foreground lane so browsing never blocks.
                    let core = core.clone();
                    covers.spawn(async move {
                        let image = core
                            .cover(CoverRequest { url: url.clone() })
                            .await
                            .ok()
                            .and_then(decode_cover);
                        (url, image)
                    });
                }
                Some(Command::RetryInit) => {}
                None => break,
            },
            changed = jobs.changed() => {
                if changed.is_err() {
                    worker_error = Some(CoreError::new(ErrorCode::Shutdown, "core snapshot stream closed"));
                    break;
                }
                snapshots.send_replace(jobs.borrow_and_update().clone());
                ctx.request_repaint();
            }
        }
    }
    commands.close();
    if let Some(cancel) = cancellation {
        cancel.send_replace(true);
    }
    // Do not abort state-changing futures: wait for enqueue/control acks, then
    // let core pause, checkpoint and join its owned transfer/remux workers.
    while foreground.join_next().await.is_some() {}
    while controls.join_next().await.is_some() {}
    // Cover fetches are read-only; dropping them cannot lose durable state.
    covers.abort_all();
    while covers.join_next().await.is_some() {}
    let report = core.shutdown(StopMode::Pause).await;
    let report = report.and_then(|report| worker_error.map_or(Ok(report), Err));
    snapshots.send_replace(jobs.borrow().clone());
    emit(&events, &ctx, Event::Stopped(report)).await;
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
        Operation::BrowseConditions(section) => core
            .browse_conditions(section)
            .await
            .map(Outcome::BrowseConditions),
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
    let specific=match (error.code,error.operation.as_str()){
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
    format!("错误码 {:?}", error.code)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }
}
