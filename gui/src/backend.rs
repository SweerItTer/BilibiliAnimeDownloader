//! Bounded native-shell adapter. Core owns all network and durable job behavior.
//! The host retains this worker and joins it before dropping its Tokio runtime.
use bili_core::{auth::QrStatus, *};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    runtime::Handle,
    sync::{mpsc, watch},
    task::{JoinHandle, JoinSet},
};

pub enum Operation {
    Search(SearchQuery),
    Resolve(String),
    Enqueue {
        selections: Vec<DownloadSelection>,
        output_dir: PathBuf,
    },
    Login {
        remember: bool,
    },
    RestoreSession,
    Logout,
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
        count: usize,
        cancelled: bool,
        error: Option<CoreError>,
    },
    Auth(AuthSnapshot),
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
        Operation::Enqueue {
            selections,
            output_dir,
        } => {
            let mut count = 0;
            for selection in selections {
                let result = tokio::select! {
                    biased;
                    _ = wait_cancel(&mut cancel) => {
                        return Ok(Outcome::Enqueued { count, cancelled: true, error: None });
                    }
                    result = core.plan(selection) => result,
                };
                let mut plan = match result {
                    Ok(plan) => plan,
                    Err(error) => {
                        return Ok(Outcome::Enqueued {
                            count,
                            cancelled: false,
                            error: Some(error),
                        })
                    }
                };
                // Keep core's safe, identity-suffixed filename and MP4 container.
                // Core fingerprints media/container, not the destination parent.
                // Only presentation's user-selected output directory changes.
                let filename = plan
                    .destination
                    .file_name()
                    .ok_or_else(|| {
                        CoreError::new(ErrorCode::InvalidInput, "core plan has no output filename")
                    })?
                    .to_owned();
                plan.destination = output_dir.join(filename);
                if *cancel.borrow() {
                    return Ok(Outcome::Enqueued {
                        count,
                        cancelled: true,
                        error: None,
                    });
                }
                // Once sent, await the durable acknowledgement even on cancel.
                if let Err(error) = core.enqueue(plan).await {
                    return Ok(Outcome::Enqueued {
                        count,
                        cancelled: false,
                        error: Some(error),
                    });
                }
                count += 1;
            }
            Ok(Outcome::Enqueued {
                count,
                cancelled: false,
                error: None,
            })
        }
        Operation::Login { remember } => login(&core, remember, &mut cancel, &events, &ctx)
            .await
            .map(Outcome::Auth),
        Operation::RestoreSession => core
            .auth(AuthAction::LoadStored { required: false })
            .await
            .map(Outcome::Auth),
        Operation::Logout => core.auth(AuthAction::Anonymous).await.map(Outcome::Auth),
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
    cancel: &mut watch::Receiver<bool>,
    events: &mpsc::Sender<Event>,
    ctx: &egui::Context,
) -> Result<AuthSnapshot> {
    let result = async {
        let start = core.auth(AuthAction::StartQr { remember }).await?;
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
            // Auth may update the session before validating it. Let it finish,
            // then clear on cancellation; never abandon half-installed auth.
            let snapshot = core.auth(AuthAction::PollQr).await?;
            if *cancel.borrow() {
                return Err(cancelled());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(CoreError::new(ErrorCode::LoginRequired, "QR login expired"));
            }
            if snapshot.logged_in {
                return Ok(snapshot);
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
        let _ = core.auth(AuthAction::CancelQr).await;
        let _ = core.auth(AuthAction::Anonymous).await;
    }
    result
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

pub fn error_message(error: &CoreError) -> String {
    let help = match error.code {
        ErrorCode::InvalidInput => "输入无效，请检查编号、链接或绝对路径。",
        ErrorCode::VerificationRequired => "站点要求安全验证；本版本不内嵌浏览器，也不会绕过验证。",
        ErrorCode::LoginRequired => "需要登录，或二维码已失效。请重新扫码。",
        ErrorCode::EntitlementRequired => "当前账号没有此内容的观看权限。",
        ErrorCode::RegionRestricted => "内容在当前地区不可用。",
        ErrorCode::Unavailable => "内容暂不可用或已下架。",
        ErrorCode::RepresentationChanged => "媒体规格已变化，请重新解析后添加。",
        ErrorCode::Transport | ErrorCode::RetryExhausted => "网络请求未成功，请检查连接后重试。",
        ErrorCode::Filesystem => "无法访问文件。请检查目录、权限和剩余空间。",
        ErrorCode::Conflict | ErrorCode::Busy => {
            "资源正在使用中。若 CLI 正在运行，请先退出再重试。"
        }
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
