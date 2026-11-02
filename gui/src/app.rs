//! Native application state. Paint only reads snapshots and sends bounded commands.
use crate::{
    backend::{self, Bridge, Command, Event, Operation, Outcome},
    background::{Background, CropEvent},
    covers::CoverCache,
    detail::DetailState,
    download_config::ConfigState,
    download_panel::QueueState,
    login::{LoginAction, LoginState},
    prefs::{BackgroundScope, GuiPrefs, StartPage, UpdateChannel, THEMES},
    search::SearchState,
    theme::{self, ButtonKind, Icon},
    update::{self, Status as UpdateStatus, UpdateState},
    window::{self, Action as WindowAction},
};
use bili_core::{AuthSnapshot, ContentRef, ErrorCode, JobId, QueueSnapshot, SearchQuery};
use egui::{RichText, Ui};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TOAST_WIDTH: f32 = 380.0;
const TOAST_ERROR_TTL: Duration = Duration::from_secs(12);
const TOAST_NOTICE_TTL: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Discover,
    Queue,
    Settings,
    About,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Search,
    Resolve,
    Enqueue,
    Login,
    Restore,
    Logout,
    Output,
    Account,
    Verification,
    Files,
}

impl Busy {
    fn label(self) -> &'static str {
        match self {
            Self::Search => "正在搜索，请稍候…",
            Self::Resolve => "正在解析内容与分集…",
            Self::Enqueue => "正在匹配画质并添加任务…",
            Self::Login => "扫码登录进行中",
            Self::Restore => "正在恢复保存的登录…",
            Self::Logout => "正在清除登录状态…",
            Self::Output => "正在检查保存位置…",
            Self::Account => "正在更新账号…",
            Self::Verification => "正在处理安全验证…",
            Self::Files => "正在处理下载文件…",
        }
    }
}

#[derive(Clone)]
enum Retry {
    Search(SearchQuery),
    Browse(bili_core::BrowseQuery),
    BrowseConditions(bili_core::PgcSection),
    Resolve(String),
    Prepare {
        inputs: Vec<backend::PrepareInput>,
        profile: Option<String>,
        refresh: bool,
    },
    Login,
    Output(PathBuf),
    Restore,
}

#[derive(Clone)]
struct Banner {
    message: String,
    hint: Option<String>,
    code: Option<String>,
    retry: Option<Retry>,
    verification: Option<bili_core::VerificationRequest>,
}

impl From<String> for Banner {
    fn from(message: String) -> Self {
        Self {
            message,
            hint: None,
            code: None,
            retry: None,
            verification: None,
        }
    }
}

impl From<&str> for Banner {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

#[derive(Clone)]
struct Notice {
    text: String,
    open_about: bool,
    open_downloads: bool,
}

impl From<String> for Notice {
    fn from(text: String) -> Self {
        Self {
            text,
            open_about: false,
            open_downloads: false,
        }
    }
}

impl From<&str> for Notice {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

pub struct App {
    bridge: Option<Bridge>,
    ready: bool,
    initializing: bool,
    init_error: Option<String>,
    page: Page,
    search: SearchState,
    detail: Option<DetailState>,
    detail_drafts: HashMap<ContentRef, DetailState>,
    config_drafts: HashMap<String, ConfigState>,
    navigation: u64,
    operation_navigation: Option<u64>,
    pending_default: Option<PathBuf>,
    awaiting_snapshot: HashSet<JobId>,
    config: ConfigState,
    account_menu: bool,
    account_manager: bool,
    auth_target: Option<JobId>,
    switching_account: bool,
    verification: Option<bili_core::VerificationChallenge>,
    verification_view: Option<crate::verification::View>,
    verification_started: Option<Instant>,
    queue: QueueState,
    login: LoginState,
    auth: Option<AuthSnapshot>,
    snapshot: Arc<QueueSnapshot>,
    busy: Option<Busy>,
    cancelling: bool,
    controls: HashSet<JobId>,
    notice: Option<Notice>,
    notice_at: Option<Instant>,
    error: Option<Banner>,
    error_at: Option<Instant>,
    covers: CoverCache,
    login_remember: bool,
    settings_jump: Option<usize>,
    retry_resolve: Option<Retry>,
    retry_discovery: Option<Retry>,
    retry_output: Option<PathBuf>,
    output: PathBuf,
    output_input: String,
    state_dir: PathBuf,
    reduced: bool,
    prefs: GuiPrefs,
    background: Background,
    prefs_dirty: bool,
    appearance: (usize, f32, bool),
    scale_draft: f32,
    confirming_close: bool,
    updates: UpdateState,
    restart: Option<std::sync::mpsc::Sender<PathBuf>>,
    app_icon: Option<egui::TextureHandle>,
    font_missing: bool,
    closing: bool,
    stopped: bool,
    allow_close: bool,
    close_error: Option<String>,
}

impl App {
    pub fn new(
        bridge: Option<Bridge>,
        output: PathBuf,
        state_dir: PathBuf,
        font_missing: bool,
        startup_error: Option<String>,
        prefs: GuiPrefs,
    ) -> Self {
        let initializing = bridge.is_some();
        let page = match prefs.start_page {
            StartPage::Discover => Page::Discover,
            StartPage::Queue => Page::Queue,
        };
        let reduced = prefs.reduced_motion;
        let appearance = (prefs.theme_index(), prefs.ui_scale, prefs.compact);
        let scale_draft = prefs.ui_scale;
        Self {
            bridge,
            ready: false,
            initializing,
            init_error: startup_error,
            page,
            search: SearchState::default(),
            detail: None,
            detail_drafts: HashMap::new(),
            config_drafts: HashMap::new(),
            navigation: 0,
            operation_navigation: None,
            pending_default: None,
            awaiting_snapshot: HashSet::new(),
            config: ConfigState::default(),
            account_menu: false,
            account_manager: false,
            auth_target: None,
            switching_account: false,
            verification: None,
            verification_view: None,
            verification_started: None,
            queue: QueueState::default(),
            login: LoginState::default(),
            auth: None,
            snapshot: Arc::new(QueueSnapshot::default()),
            busy: None,
            cancelling: false,
            controls: HashSet::new(),
            notice: None,
            notice_at: None,
            error: None,
            error_at: None,
            covers: CoverCache::default(),
            login_remember: false,
            settings_jump: None,
            retry_resolve: None,
            retry_discovery: None,
            retry_output: None,
            output_input: output.display().to_string(),
            output,
            state_dir,
            reduced,
            prefs,
            background: Background::default(),
            prefs_dirty: false,
            appearance,
            scale_draft,
            confirming_close: false,
            updates: UpdateState::default(),
            restart: None,
            app_icon: None,
            font_missing,
            closing: false,
            stopped: false,
            allow_close: false,
            close_error: None,
        }
    }

    fn touch_prefs(&mut self) {
        self.prefs_dirty = true;
    }

    pub fn attach_updates(
        &mut self,
        updates: UpdateState,
        restart: std::sync::mpsc::Sender<PathBuf>,
    ) {
        self.updates = updates;
        self.restart = Some(restart);
    }

    fn raise(&mut self, banner: impl Into<Banner>) {
        self.error = Some(banner.into());
        self.error_at = Some(Instant::now());
    }

    fn inform(&mut self, notice: impl Into<Notice>) {
        self.notice = Some(notice.into());
        self.notice_at = Some(Instant::now());
    }

    fn expire_toasts(&mut self) {
        if self
            .error_at
            .is_some_and(|shown| shown.elapsed() >= TOAST_ERROR_TTL)
            && self.error.as_ref().is_none_or(|e| e.verification.is_none())
        {
            self.error = None;
            self.error_at = None;
        }
        if self
            .notice_at
            .is_some_and(|shown| shown.elapsed() >= TOAST_NOTICE_TTL)
        {
            self.notice = None;
            self.notice_at = None;
        }
    }

    fn reset_appearance(&mut self) {
        let output = self.prefs.output.clone();
        let default_download_rate_mib = self.prefs.default_download_rate_mib;
        self.prefs = GuiPrefs {
            output,
            default_download_rate_mib,
            ..GuiPrefs::default()
        };
        self.reduced = self.prefs.reduced_motion;
        self.scale_draft = self.prefs.ui_scale;
        self.touch_prefs();
    }

    fn sync_appearance(&mut self, ctx: &egui::Context) {
        let current = (
            self.prefs.theme_index(),
            self.prefs.ui_scale,
            self.prefs.compact,
        );
        if current != self.appearance {
            self.prefs.apply(ctx);
            self.appearance = current;
        }
        let animation = if self.reduced { 0.0 } else { theme::MOTION };
        if (ctx.style().animation_time - animation).abs() > f32::EPSILON {
            let mut style = (*ctx.style()).clone();
            style.animation_time = animation;
            ctx.set_style(style);
        }
        self.background.sync(ctx, &self.prefs);
    }

    fn save_prefs(&mut self, ctx: &egui::Context) {
        if !self.prefs_dirty || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        self.prefs.save();
        self.prefs_dirty = false;
    }

    fn surface(&self, covers_image: bool) -> egui::Color32 {
        self.background
            .surface_fill(covers_image, &self.prefs, theme::background())
    }

    fn available(&self) -> bool {
        self.ready
            && self.busy.is_none()
            && self.awaiting_snapshot.is_empty()
            && !self.closing
            && !self.stopped
    }

    fn send(&mut self, operation: Operation) {
        if !self.available() {
            return;
        }
        if matches!(operation, Operation::Resolve(_) | Operation::Prepare { .. }) {
            self.operation_navigation = Some(self.navigation);
        }
        if let Operation::Enqueue { output_dir, .. } = &operation {
            self.pending_default = self.config.make_default.then(|| output_dir.clone());
        }
        if matches!(
            operation,
            Operation::Profiles | Operation::SelectProfile(_) | Operation::ForgetProfile(_)
        ) {
            self.switching_account = matches!(operation, Operation::SelectProfile(_));
        }
        let kind = match &operation {
            Operation::Search(_) => {
                self.search.start_wait();
                Busy::Search
            }
            Operation::Resolve(_) => Busy::Resolve,
            Operation::Enqueue { .. } => Busy::Enqueue,
            Operation::Prepare { .. } => Busy::Resolve,
            Operation::BrowseConditions(_) | Operation::Browse(_) => Busy::Search,
            Operation::Profiles | Operation::SelectProfile(_) | Operation::ForgetProfile(_) => {
                Busy::Account
            }
            Operation::BeginVerification { .. } | Operation::CompleteVerification { .. } => {
                Busy::Verification
            }
            Operation::OutputInfo(_) | Operation::Remove { .. } | Operation::Reveal { .. } => {
                Busy::Files
            }
            Operation::RetargetProfile { .. } => Busy::Account,
            Operation::Login { remember, .. } => {
                self.login_remember = *remember;
                Busy::Login
            }
            Operation::RestoreSession => Busy::Restore,
            Operation::Logout | Operation::Anonymous => Busy::Logout,
            Operation::ValidateOutput(_) => Busy::Output,
        };
        match &operation {
            Operation::Resolve(input) => self.retry_resolve = Some(Retry::Resolve(input.clone())),
            Operation::Prepare {
                inputs,
                profile,
                refresh,
            } => {
                self.retry_resolve = Some(Retry::Prepare {
                    inputs: inputs.clone(),
                    profile: profile.clone(),
                    refresh: *refresh,
                })
            }
            Operation::Search(query) => self.retry_discovery = Some(Retry::Search(query.clone())),
            Operation::Browse(query) => self.retry_discovery = Some(Retry::Browse(query.clone())),
            Operation::BrowseConditions(section) => {
                self.retry_discovery = Some(Retry::BrowseConditions(*section))
            }
            _ => {}
        }
        if let Operation::ValidateOutput(path) = &operation {
            self.retry_output = Some(path.clone());
        }
        if let Some(bridge) = &self.bridge {
            match bridge.send(Command::Run(operation)) {
                Ok(()) => {
                    self.busy = Some(kind);
                    self.cancelling = false;
                    self.error = None;
                    self.notice = None;
                    if kind == Busy::Login {
                        self.login.error = None;
                        self.login.clear_challenge();
                    }
                }
                Err(error) => {
                    self.raise(error);
                    if kind == Busy::Search {
                        self.search.discard_request();
                    }
                }
            }
        }
    }

    fn cancel(&mut self) {
        if self.cancelling {
            return;
        }
        if let Some(bridge) = &self.bridge {
            match bridge.send(Command::Cancel) {
                Ok(()) => {
                    self.cancelling = true;
                    if self.busy == Some(Busy::Resolve) {
                        self.operation_navigation = None;
                    }
                    self.login.clear_challenge();
                }
                Err(error) => self.raise(error),
            }
        }
    }

    fn run_retry(&mut self, retry: Retry) {
        self.error = None;
        match retry {
            Retry::Search(query) => {
                self.search.set_requested(query.clone());
                self.send(Operation::Search(query));
            }
            Retry::Browse(query) => {
                self.search.set_requested_browse(query.clone());
                self.send(Operation::Browse(query));
            }
            Retry::BrowseConditions(section) => {
                self.search.category = section.category();
                self.send(Operation::BrowseConditions(section));
            }
            Retry::Resolve(input) => self.send(Operation::Resolve(input)),
            Retry::Prepare {
                inputs,
                profile,
                refresh,
            } => self.send(Operation::Prepare {
                inputs,
                profile,
                refresh,
            }),
            Retry::Login => {
                self.login.error = None;
                self.login.open = true;
            }
            Retry::Output(path) => self.send(Operation::ValidateOutput(path)),
            Retry::Restore => self.send(Operation::RestoreSession),
        }
    }

    fn next_event(&mut self) -> Option<Event> {
        let bridge = self.bridge.as_mut()?;
        match bridge.events.try_recv() {
            Ok(event) => Some(event),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => None,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                if !self.stopped {
                    self.stopped = true;
                    self.ready = false;
                    self.initializing = false;
                    self.busy = None;
                    self.controls.clear();
                    self.login.clear_challenge();
                    self.close_error =
                        Some("后台意外结束，请保留本地状态目录并重新启动应用。".into());
                }
                None
            }
        }
    }

    fn collect(&mut self, ctx: &egui::Context) {
        if let Some(bridge) = &self.bridge {
            self.snapshot = bridge.snapshots.borrow().clone();
        }
        self.awaiting_snapshot
            .retain(|id| !self.snapshot.jobs.iter().any(|j| j.id == *id));
        while let Some(event) = self.next_event() {
            match event {
                Event::Ready => {
                    self.ready = true;
                    self.initializing = false;
                    self.init_error = None;
                    if self.prefs.remember_login {
                        self.send(Operation::RestoreSession);
                    } else {
                        self.send(Operation::Profiles);
                    }
                }
                Event::InitFailed(error) => {
                    self.ready = false;
                    self.initializing = false;
                    self.init_error = Some(backend::error_message(&error));
                }
                Event::Login(snapshot) => {
                    if !self.cancelling && !self.closing {
                        self.login.update(ctx, &snapshot);
                    }
                }
                Event::Cover { url, image } => {
                    self.covers.insert(ctx, url, image);
                }
                Event::Controlled(id, result) => {
                    self.controls.remove(&id);
                    if let Err(error) = result {
                        self.raise(control_banner(&error));
                    }
                }
                Event::Finished(result) => {
                    let switched_account =
                        self.busy == Some(Busy::Account) && self.switching_account;
                    self.switching_account = false;
                    let was_login = self.busy == Some(Busy::Login);
                    let was_restore = self.busy == Some(Busy::Restore);
                    let was_logout = self.busy == Some(Busy::Logout);
                    let was_search = self.busy == Some(Busy::Search);
                    let was_resolve = self.busy == Some(Busy::Resolve);
                    let was_output = self.busy == Some(Busy::Output);
                    // A read-only result racing with the user's Cancel must not
                    // reopen the dismissed flow or replace the committed search.
                    // Admission receipts and committed QR logins are still applied.
                    let result = if self.cancelling && (was_search || was_resolve) {
                        Err(bili_core::CoreError::new(
                            ErrorCode::Cancelled,
                            "GUI request cancelled",
                        ))
                    } else {
                        result
                    };
                    let retry_discovery = was_search.then(|| self.retry_discovery.take()).flatten();
                    let requested_navigation = if was_resolve {
                        self.operation_navigation.take()
                    } else {
                        None
                    };
                    let retry_query = if was_search {
                        self.search.requested_query().cloned()
                    } else {
                        None
                    };
                    if was_search && result.is_err() {
                        self.search.discard_request();
                        self.search.mark_stale();
                    }
                    self.busy = None;
                    self.cancelling = false;
                    if was_login {
                        self.login.clear_challenge();
                    }
                    match result {
                        Ok(Outcome::Search(results)) => {
                            self.search.accept_results(results);
                        }
                        Ok(Outcome::Detail(item)) => {
                            let mut detail = DetailState::new(item);
                            if let Some(category) =
                                self.search.result_category(&detail.item.reference)
                            {
                                detail.category = category;
                            }
                            if requested_navigation == Some(self.navigation) {
                                self.detail = self
                                    .detail_drafts
                                    .remove(&detail.item.reference)
                                    .or(Some(detail));
                            } else {
                                self.detail_drafts
                                    .entry(detail.item.reference.clone())
                                    .or_insert(detail);
                            }
                        }
                        Ok(Outcome::Enqueued {
                            receipts,
                            cancelled,
                            error,
                        }) => {
                            let count = receipts.len();
                            self.awaiting_snapshot.extend(
                                receipts
                                    .iter()
                                    .filter(|r| !self.snapshot.jobs.iter().any(|j| j.id == r.id))
                                    .map(|r| r.id),
                            );
                            let parts: Vec<_> =
                                receipts.iter().map(|r| r.plan.part.clone()).collect();
                            if let Some(detail) = &mut self.detail {
                                detail.acknowledge(&parts);
                            }
                            if let Some(directory) =
                                self.pending_default.take().filter(|_| count > 0)
                            {
                                self.output = directory;
                                self.output_input = self.output.display().to_string();
                                self.prefs.output = Some(self.output.clone());
                                self.touch_prefs();
                            }
                            self.config.acknowledge(
                                receipts,
                                cancelled,
                                error.as_ref().map(backend::error_message),
                            );
                            self.config.verification =
                                error.as_ref().and_then(|e| e.verification.clone());
                            self.inform(Notice {
                                text: format!(
                                    "已添加 {count} 项到队列。{}",
                                    if cancelled {
                                        "已停止添加后续分集；已添加的任务仍在队列中。"
                                    } else {
                                        ""
                                    }
                                ),
                                open_about: false,
                                open_downloads: count > 0,
                            });
                            if let Some(error) = error {
                                self.raise(control_banner(&error));
                            }
                        }
                        Ok(Outcome::Prepared {
                            items,
                            profile,
                            refresh,
                        }) => {
                            let open = requested_navigation == Some(self.navigation);
                            let next_key = ConfigState::intent_key(&items);
                            if !refresh && !self.config.matches_intent(&items) {
                                if let Some(key) = self.config.key() {
                                    self.config_drafts.insert(key, self.config.clone());
                                }
                                self.config =
                                    self.config_drafts.remove(&next_key).unwrap_or_default();
                            }
                            self.config.accept(
                                items,
                                profile,
                                &self.output,
                                self.prefs.default_download_rate_mib,
                                refresh,
                            );
                            self.config.exclude_admitted(
                                self.snapshot
                                    .jobs
                                    .iter()
                                    .filter_map(|j| j.plan.as_ref().map(|p| p.part.clone()))
                                    .collect(),
                            );
                            self.config.open = open;
                        }
                        Ok(Outcome::OutputInfo(info)) => self.queue.accept_info(info),
                        Ok(Outcome::Removed(report)) => {
                            self.inform(format!(
                                "已移除 {} 条记录，回收 {} 个文件。",
                                report.removed.len(),
                                report.recycled.len()
                            ));
                            if let Some(error) = report.errors.first() {
                                self.raise(control_banner(error));
                            }
                        }
                        Ok(Outcome::Retargeted(id)) => {
                            self.auth_target = None;
                            if let Some(bridge) = &self.bridge {
                                match bridge.send(Command::Control(id, bili_core::JobAction::Retry))
                                {
                                    Ok(()) => {
                                        self.controls.insert(id);
                                    }
                                    Err(error) => self.raise(error),
                                }
                            }
                            self.account_manager = false;
                            self.inform("已更换此任务的凭据并重试，其他任务保持原账号。");
                        }
                        Ok(Outcome::Revealed) => {}
                        Ok(Outcome::BrowseConditions(conditions)) => {
                            let operation = self.search.accept_conditions(conditions);
                            self.send(operation);
                        }
                        Ok(Outcome::Verification(challenge)) => {
                            self.verification = Some(challenge);
                            self.verification_started = Some(Instant::now());
                        }
                        Ok(Outcome::Verified) => self.inform("安全验证完成，可重试原操作。"),
                        Ok(Outcome::ProfileAdded { auth, profile_id }) => {
                            self.login.open = false;
                            self.login.adding = false;
                            self.auth = Some(auth);
                            self.account_manager = false;
                            if let Some(id) = self.auth_target {
                                self.send(Operation::RetargetProfile {
                                    id,
                                    profile: Some(profile_id),
                                });
                            } else {
                                self.inform("账号凭据已添加，当前使用的账号保持不变。");
                            }
                        }
                        Ok(Outcome::Auth(snapshot)) => {
                            let old_profile = self.config.effective_profile(self.auth.as_ref());
                            if was_login && snapshot.logged_in {
                                self.login.open = false;
                                self.login.adding = false;
                                if self.auth_target.is_none() {
                                    self.account_manager = false;
                                }
                                self.prefs.remember_login = self.login_remember;
                                self.touch_prefs();
                                self.inform(if self.login_remember {
                                    "登录成功，已保持登录状态。"
                                } else {
                                    "登录成功，仅在本次会话中有效。"
                                });
                            } else if was_restore {
                                if snapshot.logged_in {
                                    self.inform("已恢复保存的登录状态。");
                                } else {
                                    self.prefs.remember_login = false;
                                    self.touch_prefs();
                                    self.inform("保存的登录已失效，请重新扫码登录。");
                                }
                            } else if was_logout {
                                self.prefs.remember_login = false;
                                self.touch_prefs();
                                self.inform("当前已切换为访客。已有任务保留原账号。");
                            }
                            self.auth = Some(snapshot);
                            if switched_account {
                                self.account_manager = false;
                                self.inform("账号已切换，已有任务保留原账号。");
                            }
                            let new_profile = self.config.effective_profile(self.auth.as_ref());
                            if self.config.open && old_profile != new_profile {
                                self.send(Operation::Prepare {
                                    inputs: self.config.inputs(),
                                    profile: new_profile,
                                    refresh: true,
                                });
                            }
                        }
                        Ok(Outcome::Output(path)) => {
                            self.output_input = path.display().to_string();
                            self.output = path;
                            self.prefs.output = Some(self.output.clone());
                            self.touch_prefs();
                            self.inform("保存位置已应用到后续新任务；已有任务保持原路径。目录会在下载时创建。");
                        }
                        Err(error) => {
                            let message = backend::error_message(&error);
                            if was_login {
                                self.login.error = Some(message.clone());
                            }
                            if error.code == ErrorCode::Cancelled {
                                self.inform(message);
                            } else {
                                let mut banner = Banner::from(message);
                                banner.verification = error.verification.clone();
                                banner.code = Some(backend::error_code(&error));
                                banner.hint = if retry_query.as_ref().is_some_and(|query| {
                                    query.category == bili_core::SearchCategory::All
                                }) {
                                    Some("「全部」会同时查询视频、番剧与影视，任一分区失败时整次搜索会失败；可切换到「视频」后重试。".into())
                                } else {
                                    None
                                };
                                banner.retry = if was_search {
                                    retry_discovery.or_else(|| retry_query.map(Retry::Search))
                                } else if was_resolve {
                                    self.retry_resolve.clone()
                                } else if was_output {
                                    self.retry_output.clone().map(Retry::Output)
                                } else if was_login {
                                    Some(Retry::Login)
                                } else if was_restore {
                                    Some(Retry::Restore)
                                } else {
                                    None
                                };
                                self.raise(banner);
                            }
                        }
                    }
                }
                Event::Stopped(result) => {
                    self.stopped = true;
                    self.ready = false;
                    self.busy = None;
                    self.controls.clear();
                    self.login.clear_challenge();
                    match result {
                        Ok(_) if self.closing => {
                            self.allow_close = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Ok(_) => {
                            self.raise("后台已停止，请关闭并重新打开应用。");
                        }
                        Err(error) => self.close_error = Some(backend::error_message(&error)),
                    }
                }
            }
        }
    }

    fn begin_close(&mut self, ctx: &egui::Context) {
        self.verification_view = None;
        self.verification = None;
        self.verification_started = None;
        if self.stopped {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.closing {
            return;
        }
        self.closing = true;
        self.confirming_close = false;
        self.prefs.save();
        self.prefs_dirty = false;
        self.login.open = false;
        self.login.clear_challenge();
        if let Some(bridge) = &self.bridge {
            bridge.shutdown();
        } else {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn navigate(&mut self, page: Page) {
        self.navigation = self.navigation.wrapping_add(1);
        self.page = page;
        self.config.open = false;
    }
    fn configuration(&mut self, ctx: &egui::Context) {
        let was_open = self.config.open;
        if let Some(action) = self.config.show(ctx, self.available(), self.auth.as_ref()) {
            self.send(action);
        }
        if was_open && !self.config.open {
            // Closing a dialog is also an explicit navigation decision. A
            // pending account/catalog refresh must not reopen it on completion.
            self.operation_navigation = None;
        }
        if self.config.history_requested {
            self.config.history_requested = false;
            self.navigate(Page::Queue);
        }
    }
    fn home(&mut self) {
        if let Some(detail) = self.detail.take() {
            self.detail_drafts
                .insert(detail.item.reference.clone(), detail);
        }
        self.navigate(Page::Discover);
    }
    fn header(&mut self, ctx: &egui::Context) {
        if self.config.open {
            self.account_menu = false;
        }
        let available = self.available();
        let mut action = None;
        let mut avatar_rect = egui::Rect::NOTHING;
        egui::TopBottomPanel::top("browse-header")
            .frame(
                egui::Frame::none()
                    .fill(self.surface(self.background.visible_for(&self.prefs, true)))
                    .inner_margin(egui::vec2(18.0, 10.0)),
            )
            .show(ctx, |ui| {
                if self.config.open {
                    ui.disable();
                }
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("BiliDL").size(25.0).strong())
                                .frame(false),
                        )
                        .on_hover_text("返回首页")
                        .clicked()
                    {
                        self.home();
                    }
                    if let Some(next) = self.search.header(ui, available) {
                        self.home();
                        action = Some(next);
                    }
                    if self.search.home_requested {
                        self.home();
                        self.search.home_requested = false;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if circle_action(ui, Icon::Download, "下载历史").clicked() {
                            self.navigate(Page::Queue);
                        }
                        let response = circle_action(ui, Icon::User, "账号菜单");
                        if let Some(face) = self
                            .auth
                            .as_ref()
                            .and_then(|a| a.account.as_ref())
                            .and_then(|a| a.face.as_deref())
                            .map(normalize_face)
                        {
                            self.covers.ensure(self.bridge.as_ref(), &face);
                            if let Some(id) = self.covers.texture(&face) {
                                ui.put(
                                    response.rect.shrink(1.0),
                                    egui::Image::new((id, response.rect.shrink(1.0).size()))
                                        .rounding(18.0),
                                );
                            }
                        }
                        avatar_rect = response.rect;
                        if response.clicked() {
                            self.account_menu = !self.account_menu;
                        }
                        ui.add_space(24.0);
                        let width = (ui.available_width() - 70.0).clamp(90.0, 470.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(width + 65.0, 36.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                if let Some(next) = self.search.input(ui, available, width) {
                                    self.home();
                                    action = Some(next);
                                }
                            },
                        );
                    });
                });
            });
        if let Some(action) = action {
            self.send(action);
        }
        if self.account_menu {
            let shown = egui::Area::new(egui::Id::new("account-menu"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(
                    (avatar_rect.right() - 235.0).max(12.0),
                    avatar_rect.bottom() + 8.0,
                ))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(235.0);
                        let name = self
                            .auth
                            .as_ref()
                            .and_then(|a| a.account.as_ref())
                            .map_or("访客", |a| a.name.as_str());
                        ui.label(RichText::new(name).strong());
                        if ui.button("切换账号").clicked() {
                            self.account_manager = true;
                            self.account_menu = false;
                            self.send(Operation::Profiles);
                        }
                        if ui
                            .add_enabled(self.available(), egui::Button::new("添加账号"))
                            .clicked()
                        {
                            self.login.begin_add();
                            self.account_menu = false;
                        }
                        if self.auth.as_ref().is_some_and(|a| a.logged_in)
                            && ui.button("退出登录").clicked()
                        {
                            self.send(Operation::Logout);
                            self.account_menu = false;
                        }
                        ui.separator();
                        if ui.button("设置").clicked() {
                            self.navigate(Page::Settings);
                            self.account_menu = false;
                        }
                        if ui.button("版本与更新").clicked() {
                            self.navigate(Page::About);
                            self.account_menu = false;
                        }
                    });
                });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape))
                || ctx.input(|i| {
                    i.pointer.any_pressed()
                        && i.pointer.interact_pos().is_some_and(|p| {
                            !shown.response.rect.contains(p) && !avatar_rect.contains(p)
                        })
                })
            {
                self.account_menu = false;
            }
        }
    }

    fn verification_dialog(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(challenge) = &self.verification else {
            return;
        };
        if self
            .verification_started
            .is_some_and(|start| start.elapsed() >= Duration::from_secs(challenge.expires_in_secs))
        {
            self.verification_view = None;
            self.verification = None;
            self.verification_started = None;
            self.raise("安全验证已超时，请重试原操作获取新验证。");
            return;
        }
        if self.verification_view.is_none() {
            match crate::verification::View::new(frame, challenge, ctx.clone()) {
                Ok(view) => self.verification_view = Some(view),
                Err(error) => {
                    self.verification = None;
                    self.verification_started = None;
                    self.raise(error);
                    return;
                }
            }
        }
        let mut open = true;
        let mut cancelled = false;
        let mut bounds = egui::Rect::NOTHING;
        egui::Window::new("安全验证")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(640.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("完成验证后，可返回重试原操作。");
                bounds = ui
                    .allocate_exact_size(
                        egui::vec2(ui.available_width().min(640.0), 360.0),
                        egui::Sense::hover(),
                    )
                    .0;
                cancelled = ui.button("取消验证").clicked();
            });
        if !open || cancelled || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.verification_view = None;
            self.verification = None;
            self.verification_started = None;
            self.inform("已取消验证，原任务保持失败状态。");
            return;
        }
        if let Some(view) = &mut self.verification_view {
            if let Err(error) = view.set_bounds(bounds, ctx.pixels_per_point()) {
                self.verification_view = None;
                self.verification = None;
                self.verification_started = None;
                self.raise(error);
                return;
            }
            if let Some(result) = view.poll() {
                self.verification_view = None;
                self.verification_started = None;
                let challenge = self
                    .verification
                    .take()
                    .expect("owned verification challenge");
                match result {
                    Ok(proof) => self.send(Operation::CompleteVerification { challenge, proof }),
                    Err(error) => self.raise(error),
                }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn accounts(&mut self, ctx: &egui::Context) {
        if !self.account_manager || self.config.open {
            return;
        }
        let mut open = self.account_manager;
        let mut action = None;
        egui::Window::new(if self.auth_target.is_some() {
            "选择此任务的账号"
        } else {
            "切换账号"
        })
        .open(&mut open)
        .collapsible(false)
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            if let Some(auth) = &self.auth {
                for profile in &auth.profiles {
                    ui.group(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(&profile.account.name);
                            theme::muted(
                                ui,
                                match profile.account.vip {
                                    Some(true) => "大会员",
                                    Some(false) => "普通账号",
                                    None => "会员状态未知",
                                },
                            );
                            let selected = auth.selected_profile.as_ref() == Some(&profile.id);
                            if ui
                                .add_enabled(
                                    self.available(),
                                    egui::Button::new(if self.auth_target.is_some() {
                                        "使用并重试"
                                    } else if selected {
                                        "当前账号"
                                    } else {
                                        "使用"
                                    }),
                                )
                                .clicked()
                            {
                                action = Some(if let Some(id) = self.auth_target {
                                    Operation::RetargetProfile {
                                        id,
                                        profile: Some(profile.id.clone()),
                                    }
                                } else {
                                    Operation::SelectProfile(profile.id.clone())
                                });
                            }
                            if self.auth_target.is_none()
                                && ui
                                    .add_enabled(self.available(), egui::Button::new("移除凭据"))
                                    .clicked()
                            {
                                action = Some(Operation::ForgetProfile(profile.id.clone()));
                            }
                        });
                        theme::muted(
                            ui,
                            format!(
                                "凭据状态：{} · {}",
                                match profile.validity {
                                    bili_core::auth::ProfileValidity::Valid => "有效",
                                    bili_core::auth::ProfileValidity::Expired => "已过期",
                                    bili_core::auth::ProfileValidity::Unknown => "未验证",
                                },
                                if profile.remembered {
                                    "已记住"
                                } else {
                                    "仅本次会话"
                                }
                            ),
                        );
                        theme::muted(
                            ui,
                            profile.expires_at.map_or_else(
                                || "登录有效期：站点未提供".into(),
                                |time| {
                                    if time / 1000 <= now_epoch() {
                                        "登录凭据已过期".into()
                                    } else {
                                        format!(
                                            "距登录凭据到期还有 {} 小时",
                                            (time / 1000 - now_epoch()).div_ceil(3600)
                                        )
                                    }
                                },
                            ),
                        );
                    });
                }
                if auth.profiles.is_empty() {
                    theme::muted(ui, "没有可选的登录凭据。");
                }
            }
            if ui
                .add_enabled(self.available(), egui::Button::new("添加账号"))
                .clicked()
            {
                self.login.begin_add();
            }
            if self.auth_target.is_none() && ui.button("使用访客").clicked() {
                action = Some(Operation::Anonymous);
            }
        });
        self.account_manager = open;
        if let Some(action) = action {
            self.send(action);
        }
    }

    fn banners(&mut self, ui: &mut Ui) {
        if self.font_missing {
            theme::inset(theme::rose()).show(ui, |ui| {
                ui.label("Chinese font not found. Install Microsoft YaHei or Noto Sans CJK, then restart Bili.");
            });
        }
        if self.initializing {
            theme::loading(ui, self.reduced, "正在打开本地下载记录…");
        }
        if let Some(error) = self.init_error.clone() {
            theme::inset(theme::rose()).show(ui, |ui| {
                ui.colored_label(theme::ERROR, "下载核心暂不可用，仍可浏览界面和调整设置。");
                ui.label(error);
                ui.add_enabled_ui(
                    self.bridge.is_some() && !self.initializing && !self.closing,
                    |ui| {
                        if theme::button(
                            ui,
                            "重试初始化",
                            Icon::Refresh,
                            ButtonKind::Danger,
                            self.reduced,
                        )
                        .clicked()
                        {
                            if let Some(bridge) = &self.bridge {
                                match bridge.send(Command::RetryInit) {
                                    Ok(()) => self.initializing = true,
                                    Err(error) => self.raise(error),
                                }
                            }
                        }
                    },
                );
            });
        }
        ui.add_space(8.0);
    }

    fn busy_label(&self, busy: Busy) -> String {
        if busy == Busy::Search {
            self.search.waited().map_or_else(
                || busy.label().to_owned(),
                |waited| format!("正在搜索，已等待 {} 秒…", waited.as_secs()),
            )
        } else {
            busy.label().to_owned()
        }
    }

    fn busy_toast(&mut self, ui: &mut Ui, busy: Busy) {
        theme::inset(theme::soft()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let label = if self.cancelling {
                "正在结束当前操作，请稍候…".to_owned()
            } else {
                self.busy_label(busy)
            };
            theme::loading(ui, self.reduced, &label);
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if matches!(
                    busy,
                    Busy::Search | Busy::Resolve | Busy::Enqueue | Busy::Login
                ) {
                    ui.add_enabled_ui(!self.cancelling, |ui| {
                        if theme::button(
                            ui,
                            "取消操作",
                            Icon::Close,
                            ButtonKind::Quiet,
                            self.reduced,
                        )
                        .clicked()
                        {
                            self.cancel();
                        }
                    });
                }
                if busy == Busy::Login
                    && !self.login.open
                    && theme::button(ui, "查看二维码", Icon::User, ButtonKind::Soft, self.reduced)
                        .clicked()
                {
                    self.login.open = true;
                }
            });
            if busy == Busy::Enqueue {
                theme::muted(ui, "已加入的任务不会因取消添加而停止");
            }
        });
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        let notice_visible = self.notice.as_ref().is_some_and(|notice| {
            !(notice.open_about && (self.error.is_some() || self.page == Page::About))
        });
        if self.error.is_none() && !notice_visible && self.busy.is_none() {
            return;
        }
        egui::Area::new(egui::Id::new("app-toasts"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-18.0, 58.0))
            .show(ctx, |ui| {
                ui.set_width(TOAST_WIDTH);
                if let Some(busy) = self.busy {
                    self.busy_toast(ui, busy);
                    ui.add_space(8.0);
                }
                if let Some(banner) = self.error.clone() {
                    theme::inset(theme::rose()).show(ui, |ui| {
                        if self.config.open {
                            ui.disable();
                        }
                        ui.set_width(ui.available_width());
                        ui.horizontal_top(|ui| {
                            let width = (ui.available_width() - 30.0).max(120.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(width, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    let message = ui.label(RichText::new(&banner.message).strong());
                                    if let Some(code) = &banner.code {
                                        message.on_hover_text(format!(
                                            "{code} · 提示不包含站点返回内容或凭据"
                                        ));
                                    }
                                    if let Some(hint) = &banner.hint {
                                        ui.label(
                                            RichText::new(hint)
                                                .size(12.0)
                                                .color(theme::muted_color()),
                                        );
                                    }
                                },
                            );
                            if toast_close(ui) {
                                self.error = None;
                                self.error_at = None;
                            }
                        });
                        if let Some(retry) = banner.retry.clone() {
                            ui.add_space(4.0);
                            if theme::button(
                                ui,
                                "重试",
                                Icon::Refresh,
                                ButtonKind::Soft,
                                self.reduced,
                            )
                            .clicked()
                            {
                                self.run_retry(retry);
                            }
                        }
                        if let Some(request) = banner.verification.clone() {
                            if ui
                                .add_enabled(self.available(), egui::Button::new("安全验证"))
                                .clicked()
                            {
                                self.send(Operation::BeginVerification {
                                    request,
                                    profile: self
                                        .auth
                                        .as_ref()
                                        .and_then(|a| a.selected_profile.clone()),
                                });
                            }
                        }
                    });
                    ui.add_space(8.0);
                }
                if let Some(notice) = self.notice.clone().filter(|notice| {
                    !(notice.open_about && (self.error.is_some() || self.page == Page::About))
                }) {
                    theme::inset(theme::soft()).show(ui, |ui| {
                        if self.config.open {
                            ui.disable();
                        }
                        ui.set_width(ui.available_width());
                        ui.horizontal_top(|ui| {
                            let width = (ui.available_width() - 30.0).max(120.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(width, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.label(notice.text.clone());
                                },
                            );
                            if toast_close(ui) {
                                self.notice = None;
                                self.notice_at = None;
                            }
                        });
                        if notice.open_about {
                            ui.add_space(4.0);
                            if theme::button(
                                ui,
                                "查看",
                                Icon::Arrow,
                                ButtonKind::Soft,
                                self.reduced,
                            )
                            .clicked()
                            {
                                self.navigate(Page::About);
                                self.notice = None;
                                self.notice_at = None;
                            }
                        }
                        if notice.open_downloads && ui.button("查看下载").clicked() {
                            self.navigate(Page::Queue);
                            self.notice = None;
                            self.notice_at = None;
                        }
                    });
                }
            });
    }

    fn settings_nav(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            theme::muted(ui, "快速跳转");
            for (index, name) in [
                (0, "外观"),
                (1, "背景"),
                (2, "下载"),
                (3, "界面"),
                (4, "隐私"),
            ] {
                if theme::button(ui, name, Icon::Arrow, ButtonKind::Quiet, self.reduced).clicked() {
                    self.settings_jump = Some(index);
                }
            }
        });
        ui.add_space(2.0);
    }

    fn settings(&mut self, ui: &mut Ui) {
        if ui.button("恢复默认外观").clicked() {
            self.reset_appearance();
        }
        jump_to(ui, 0, &mut self.settings_jump);
        theme::card_frame().show(ui, |ui| {
            theme::inset(theme::card()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::section(ui, "主题", "内置 5 套配色，切换后立即生效并记住选择。");
                ui.horizontal_wrapped(|ui| {
                    for (index, preset) in THEMES.iter().enumerate() {
                        let selected = self.prefs.theme_index() == index;
                        if theme::button(
                            ui,
                            preset.name,
                            if selected { Icon::Check } else { Icon::Palette },
                            if selected {
                                ButtonKind::Soft
                            } else {
                                ButtonKind::Quiet
                            },
                            self.reduced,
                        )
                        .clicked()
                            && !selected
                        {
                            self.prefs.set_theme(index);
                            self.touch_prefs();
                        }
                    }
                });
                ui.add_space(18.0);
                ui.separator();
                ui.add_space(14.0);
                jump_to(ui, 1, &mut self.settings_jump);
                theme::section(ui, "背景图", "只从本机读取图片，不上传、不联网；文件上限 64 MiB。");
                let path_label = self
                    .prefs
                    .background
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "尚未选择背景图片".into());
                theme::hint(ui, path_label);
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if theme::button(ui, "选择图片", Icon::Image, ButtonKind::Soft, self.reduced)
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("图片", &["png", "jpg", "jpeg", "webp"])
                            .set_title("选择背景图片")
                            .pick_file()
                        {
                            self.prefs.background = Some(path);
                            self.prefs.crop = [0.0, 0.0, 1.0, 1.0];
                            self.background.crop_open = false;
                            self.touch_prefs();
                        }
                    }
                    ui.add_enabled_ui(self.prefs.background.is_some(), |ui| {
                        if theme::button(
                            ui,
                            "裁剪图片",
                            Icon::Crop,
                            ButtonKind::Quiet,
                            self.reduced,
                        )
                        .clicked()
                        {
                            self.background.open_crop(ui.ctx(), &self.prefs);
                        }
                        if theme::button(
                            ui,
                            "移除背景",
                            Icon::Close,
                            ButtonKind::Quiet,
                            self.reduced,
                        )
                        .clicked()
                        {
                            self.prefs.background = None;
                            self.prefs.crop = [0.0, 0.0, 1.0, 1.0];
                            self.background.crop_open = false;
                            self.touch_prefs();
                        }
                    });
                });
                if let Some(error) = self.background.error() {
                    ui.colored_label(theme::ERROR, error);
                }
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    theme::muted(ui, "铺放范围");
                    for (scope, name) in [
                        (BackgroundScope::Full, "整个窗口"),
                        (BackgroundScope::WithoutSidebar, "仅内容区"),
                    ] {
                        let selected = self.prefs.background_scope == scope;
                        let response = theme::button(
                            ui,
                            name,
                            if selected { Icon::Check } else { Icon::Image },
                            if selected {
                                ButtonKind::Soft
                            } else {
                                ButtonKind::Quiet
                            },
                            self.reduced,
                        );
                        if response.clicked() && !selected {
                            self.prefs.background_scope = scope;
                            self.touch_prefs();
                        }
                    }
                });
                ui.add_space(8.0);
                let mut opacity = self.prefs.opacity;
                let opacity_response = ui.add(
                    egui::Slider::new(&mut opacity, 0.0..=100.0)
                        .text("背景不透明度")
                        .suffix("%"),
                );
                if opacity_response.changed() {
                    self.prefs.opacity = opacity;
                    self.touch_prefs();
                }
                ui.add_space(4.0);
                let mut blur = self.prefs.blur;
                let blur_response = ui.add(
                    egui::Slider::new(&mut blur, 0.0..=100.0)
                        .text("背景模糊程度")
                        .suffix("%"),
                );
                if blur_response.changed() {
                    self.prefs.blur = blur;
                    self.touch_prefs();
                }
                ui.add_space(18.0);
                ui.separator();
                ui.add_space(14.0);
                jump_to(ui, 2, &mut self.settings_jump);
                theme::section(
                    ui,
                    "下载位置",
                    "只影响后续新任务；已有任务保持原路径。请填写或选择绝对目录。",
                );
                ui.add(
                    theme::singleline_input(ui,&mut self.output_input,"",ui.spacing().interact_size.y)
                        .desired_width(f32::INFINITY)
                        .margin(egui::vec2(12.0, 12.0)),
                );
                let path = PathBuf::from(self.output_input.trim());
                let valid = !self.output_input.trim().is_empty() && path.is_absolute();
                if !valid {
                    ui.colored_label(
                        theme::ERROR,
                        "请输入绝对路径，例如 C:\\Users\\你的用户名\\Downloads 或 /home/user/Downloads。",
                    );
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if theme::button(
                        ui,
                        "选择文件夹",
                        Icon::Folder,
                        ButtonKind::Quiet,
                        self.reduced,
                    )
                    .clicked()
                    {
                        if let Some(dir) =
                            rfd::FileDialog::new().set_title("选择下载目录").pick_folder()
                        {
                            self.output_input = dir.display().to_string();
                        }
                    }
                    ui.add_enabled_ui(self.available() && valid && path != self.output, |ui| {
                        if theme::button(
                            ui,
                            "应用保存位置",
                            Icon::Check,
                            ButtonKind::Primary,
                            self.reduced,
                        )
                        .clicked()
                        {
                            self.send(Operation::ValidateOutput(path.clone()));
                        }
                    });
                });
                ui.add_space(6.0);
                theme::hint(ui, format!("当前生效：{}", self.output.display()));
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("默认下载限速");
                    if ui
                        .add(
                            egui::DragValue::new(&mut self.prefs.default_download_rate_mib)
                                .range(0.0..=crate::prefs::MAX_DOWNLOAD_RATE_MIB)
                                .speed(0.1)
                                .suffix(" MiB/s"),
                        )
                        .changed()
                    {
                        self.prefs.default_download_rate_mib = crate::prefs::sanitize_download_rate(
                            self.prefs.default_download_rate_mib,
                        );
                        self.touch_prefs();
                    }
                    theme::muted(ui, "0 不限速");
                });
                theme::hint(ui, "新下载使用此默认值，可在下载时展开高级设置单独修改。");
                ui.add_space(18.0);
                ui.separator();
                ui.add_space(14.0);
                jump_to(ui, 3, &mut self.settings_jump);
                theme::section(ui, "界面与动效", "这些偏好只影响界面显示，不改变下载内容。");
                if ui.checkbox(&mut self.reduced, "减少动态效果").changed() {
                    self.prefs.reduced_motion = self.reduced;
                    self.touch_prefs();
                }
                theme::hint_indented(
                    ui,
                    "hint-reduced",
                    "关闭循环与过渡动画；状态与进度仍会实时更新。",
                );
                ui.add_space(12.0);
                if ui.checkbox(&mut self.prefs.compact, "紧凑列表").changed() {
                    self.touch_prefs();
                }
                theme::hint_indented(
                    ui,
                    "hint-compact",
                    "缩小间距与按钮高度，适合小屏或任务较多时。",
                );
                ui.add_space(12.0);
                let mut scale = self.scale_draft;
                ui.add(
                    egui::Slider::new(&mut scale, 0.8..=1.3)
                        .text("界面缩放")
                        .fixed_decimals(2),
                );
                self.scale_draft = scale;
                let scale_pending = (self.scale_draft - self.prefs.ui_scale).abs() > 0.001;
                theme::hint_indented(
                    ui,
                    "hint-scale",
                    if scale_pending {
                        format!(
                            "当前生效 {:.2}，点击「应用缩放」后才会改变界面。",
                            self.prefs.ui_scale
                        )
                    } else {
                        "拖动滑块只调整预览值，应用后才生效。".to_owned()
                    },
                );
                if scale_pending {
                    ui.add_space(6.0);
                    if theme::button(
                        ui,
                        "应用缩放",
                        Icon::Check,
                        ButtonKind::Soft,
                        self.reduced,
                    )
                    .clicked()
                    {
                        self.prefs.ui_scale = self.scale_draft;
                        self.touch_prefs();
                    }
                }
                ui.add_space(14.0);
                ui.horizontal_wrapped(|ui| {
                    theme::hint(ui, "启动页面");
                    for (page, name) in [(StartPage::Discover, "发现"), (StartPage::Queue, "队列")] {
                        let selected = self.prefs.start_page == page;
                        if theme::button(
                            ui,
                            name,
                            if selected { Icon::Check } else { Icon::Arrow },
                            if selected {
                                ButtonKind::Soft
                            } else {
                                ButtonKind::Quiet
                            },
                            self.reduced,
                        )
                        .clicked()
                            && !selected
                        {
                            self.prefs.start_page = page;
                            self.touch_prefs();
                        }
                    }
                });
                ui.add_space(10.0);
                if ui
                    .checkbox(&mut self.prefs.confirm_close, "关闭窗口前先确认")
                    .changed()
                {
                    self.touch_prefs();
                }
                ui.add_space(18.0);
                ui.separator();
                ui.add_space(14.0);
                jump_to(ui, 4, &mut self.settings_jump);
                theme::section(
                    ui,
                    "本地与隐私",
                    "偏好保存到系统配置目录，不包含账号、Cookie 或二维码。",
                );
                theme::hint(ui, "启动时只读取本地记录，不自动搜索、登录或恢复下载。");
                ui.add_space(2.0);
                theme::hint(ui, "登录凭据按登录方式选择内存或系统凭据库。关闭窗口时暂停任务并保存可恢复状态。");
                ui.add_space(2.0);
                theme::hint(ui, "画质协商、下载、MP4 封装与字幕 / 弹幕处理均由 bili_core 完成。");
                ui.add_space(8.0);
                theme::hint(ui, format!("状态目录：{}", self.state_dir.display()));
                theme::hint(
                    ui,
                    "需要隔离记录时，以 --state-dir <绝对目录> 启动。不要同时用 CLI 写入同一状态目录。",
                );
            });
        });
    }

    fn ensure_icon(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        if self.app_icon.is_none() {
            let image = image::load_from_memory(include_bytes!("../assets/icon-256.png"))
                .ok()?
                .into_rgba8();
            let (w, h) = image.dimensions();
            let color =
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], image.as_raw());
            self.app_icon = Some(ctx.load_texture("app-icon", color, egui::TextureOptions::LINEAR));
        }
        self.app_icon.as_ref().map(|texture| texture.id())
    }

    fn about(&mut self, ui: &mut Ui) {
        let wide = ui.available_width() >= 900.0;
        if wide {
            let gap = 14.0;
            let column = ((ui.available_width() - gap) / 2.0).max(320.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(column, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.about_brand(ui),
                );
                ui.add_space(gap);
                ui.allocate_ui_with_layout(
                    egui::vec2(column, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.about_info(ui),
                );
            });
        } else {
            self.about_brand(ui);
            ui.add_space(14.0);
            self.about_info(ui);
        }
        if self.updates.status().is_checking() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn about_brand(&mut self, ui: &mut Ui) {
        let icon = self.ensure_icon(ui.ctx());
        let status = self.updates.status().clone();
        theme::card_frame().show(ui, |ui| {
            theme::inset(theme::card()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.add_space(6.0);
                    if let Some(id) = icon {
                        ui.add(egui::Image::new((id, egui::vec2(96.0, 96.0))).rounding(24.0));
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("Bili").size(30.0).strong());
                    theme::muted(ui, format!("v{} · 原生桌面下载器", update::VERSION));
                    ui.add_space(8.0);
                    match &status {
                        UpdateStatus::Checking => {
                            theme::loading(ui, self.reduced, "正在检查更新…");
                        }
                        UpdateStatus::Downloading(release) => {
                            let label = format!("正在下载 {}…", release.tag);
                            theme::loading(ui, self.reduced, &label);
                        }
                        UpdateStatus::Available(release) => {
                            ui.label(
                                RichText::new(format!("发现新版本 {}", release.tag))
                                    .color(theme::primary())
                                    .strong(),
                            );
                        }
                        UpdateStatus::Ready { version } => {
                            ui.label(
                                RichText::new(format!("v{version} 已安装，重启后生效"))
                                    .color(theme::GREEN),
                            );
                        }
                        UpdateStatus::Error(message) => {
                            ui.colored_label(theme::ERROR, message);
                        }
                        UpdateStatus::UpToDate => {
                            ui.label(RichText::new("已是最新版本").color(theme::GREEN));
                        }
                        UpdateStatus::NoCompatibleRelease => {
                            theme::muted(ui, "更高版本暂无适用于当前平台的 GUI 包");
                        }
                        UpdateStatus::Idle => {
                            theme::muted(ui, "尚未检查更新");
                        }
                    }
                    ui.add_space(10.0);
                    let check_label = match &status {
                        UpdateStatus::Checking => "正在检查",
                        UpdateStatus::Downloading(_) => "正在下载",
                        _ => "检查更新",
                    };
                    let extra = match &status {
                        UpdateStatus::Available(_) => Some(("下载并安装", Icon::Download)),
                        UpdateStatus::Ready { .. } => Some(("重启使用新版本", Icon::Refresh)),
                        _ => None,
                    };
                    let mut row_width = theme::button_width_for(ui, check_label);
                    if let Some((label, _)) = extra {
                        row_width += ui.spacing().item_spacing.x + theme::button_width_for(ui, label);
                    }
                    let mut apply_update = false;
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(((ui.available_width() - row_width) / 2.0).max(0.0));
                        if theme::button(
                            ui,
                            check_label,
                            Icon::Refresh,
                            ButtonKind::Primary,
                            self.reduced,
                        )
                        .clicked()
                            && !status.is_checking()
                        {
                            self.updates.start(self.prefs.update_channel());
                        }
                        match (&status, extra) {
                            (UpdateStatus::Available(_), Some((label, symbol))) => {
                                if theme::button(
                                    ui,
                                    label,
                                    symbol,
                                    ButtonKind::Soft,
                                    self.reduced,
                                )
                                .clicked()
                                {
                                    self.updates.install();
                                }
                            }
                            (UpdateStatus::Ready { .. }, Some((label, symbol))) => {
                                apply_update = theme::button(
                                    ui,
                                    label,
                                    symbol,
                                    ButtonKind::Soft,
                                    self.reduced,
                                )
                                .clicked();
                            }
                            _ => {}
                        }
                    });
                    if apply_update {
                        self.restart_for_update(ui.ctx());
                    }
                    ui.add_space(10.0);
                    theme::inset(theme::background()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        theme::muted(
                            ui,
                            "从静态更新目录检查新版本，不上传账号、Cookie 或下载记录。安装包通过大小、SHA-256、身份和运行库检查后，安装到独立版本目录；当前任务继续运行，重启后使用新版本。",
                        );
                        ui.label(
                            RichText::new(format!(
                                "上次检查：{}",
                                checked_ago(self.prefs.last_check)
                            ))
                            .size(12.0)
                            .color(theme::muted_color()),
                        );
                    });
                });
            });
        });
    }

    fn restart_for_update(&mut self, ctx: &egui::Context) {
        if !matches!(self.updates.status(), UpdateStatus::Ready { .. }) {
            return;
        }
        match update::restart_entry().and_then(|entry| {
            self.restart
                .as_ref()
                .ok_or_else(|| "无法请求重启，请关闭后手动打开应用。".to_owned())?
                .send(entry)
                .map_err(|_| "无法请求重启，请关闭后手动打开应用。".to_owned())
        }) {
            Ok(()) => self.begin_close(ctx),
            Err(error) => self.raise(error),
        }
    }

    fn about_info(&mut self, ui: &mut Ui) {
        theme::card_frame().show(ui, |ui| {
            theme::inset(theme::card()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("更新设置").size(18.0).strong());
                if ui
                    .checkbox(&mut self.prefs.update.check_updates, "自动检查更新（每天最多一次）")
                    .changed()
                {
                    self.touch_prefs();
                    if self.prefs.update.check_updates && !self.updates.status().is_checking() {
                        self.updates.automatic(self.prefs.update_channel());
                    }
                }
                ui.add_space(8.0);
                if ui
                    .checkbox(
                        &mut self.prefs.update.auto_update,
                        "自动安装更新（重启应用后生效）",
                    )
                    .changed()
                {
                    self.touch_prefs();
                }
                theme::hint_indented(
                    ui,
                    "hint-auto-update",
                    "发现新版本后自动下载并校验，校验通过后安装到独立版本目录。当前任务继续运行，下次启动使用新版本。",
                );
                ui.add_space(6.0);
                theme::muted(ui, "更新通道决定检查稳定版还是包含预发布的 Beta。");
                ui.horizontal_wrapped(|ui| {
                    theme::muted(ui, "更新通道");
                    for (channel, name) in [
                        (UpdateChannel::Stable, "稳定版"),
                        (UpdateChannel::Beta, "Beta"),
                    ] {
                        let selected = self.prefs.update_channel() == channel;
                        if theme::button(
                            ui,
                            name,
                            if selected { Icon::Check } else { Icon::Arrow },
                            if selected {
                                ButtonKind::Soft
                            } else {
                                ButtonKind::Quiet
                            },
                            self.reduced,
                        )
                        .clicked()
                            && !selected
                        {
                            self.prefs.update.allow_pre_release = channel == UpdateChannel::Beta;
                            self.touch_prefs();
                        }
                    }
                });
                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);
                ui.label(RichText::new("项目与支持").size(18.0).strong());
                ui.horizontal_wrapped(|ui| {
                    if theme::button(
                        ui,
                        "GitHub 仓库",
                        Icon::Arrow,
                        ButtonKind::Quiet,
                        self.reduced,
                    )
                    .clicked()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(update::REPO_URL));
                    }
                    if theme::button(ui, "反馈问题", Icon::Arrow, ButtonKind::Quiet, self.reduced)
                        .clicked()
                    {
                        ui.ctx()
                            .open_url(egui::OpenUrl::new_tab(update::ISSUES_URL));
                    }
                });
                ui.label(
                    RichText::new(update::REPO_URL)
                        .size(12.0)
                        .color(theme::primary()),
                );
                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);
                ui.label(RichText::new("运行信息").size(18.0).strong());
                theme::muted(
                    ui,
                    "登录凭据按登录方式选择内存或系统凭据库；关闭窗口时暂停任务并保存断点。",
                );
                ui.label(
                    RichText::new(format!("状态目录：{}", self.state_dir.display()))
                        .size(12.0)
                        .color(theme::muted_color()),
                );
            });
        });
    }

    fn central(&mut self, ctx: &egui::Context) {
        self.expire_toasts();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(self.surface(true))
                    .inner_margin(18.0),
            )
            .show(ctx, |ui| {
                if self.config.open {
                    ui.disable();
                }
                self.banners(ui);
                if self.page == Page::Settings && !self.background.crop_open {
                    self.settings_nav(ui);
                }
                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt(match self.page {
                        Page::Discover if self.detail.is_some() => "main-scroll-detail",
                        Page::Discover => "main-scroll-discover",
                        Page::Queue => "main-scroll-queue",
                        Page::Settings => "main-scroll-settings",
                        Page::About => "main-scroll-about",
                    })
                    .auto_shrink([false, false]);
                if self.background.crop_open {
                    scroll = scroll.vertical_scroll_offset(0.0);
                }
                scroll.show(ui, |ui| {
                    let available = self.available();
                    match self.page {
                        Page::Discover => {
                            let action = if let Some(detail) = &mut self.detail {
                                let (back, action) = detail.show(
                                    ui,
                                    available,
                                    self.reduced,
                                    &self.output,
                                    &mut self.covers,
                                    self.bridge.as_ref(),
                                );
                                if back {
                                    self.home();
                                }
                                action
                            } else {
                                self.search.show(
                                    ui,
                                    available,
                                    self.reduced,
                                    &mut self.covers,
                                    self.bridge.as_ref(),
                                )
                            };
                            if let Some(mut action) = action {
                                if let Operation::Prepare { profile, .. } = &mut action {
                                    *profile =
                                        self.auth.as_ref().and_then(|a| a.selected_profile.clone());
                                }
                                self.send(action);
                            }
                        }
                        Page::Queue => {
                            let action = self.queue.show(
                                ui,
                                crate::download_panel::QueueView {
                                    snapshot: &self.snapshot,
                                    pending: &self.controls,
                                    available: self.ready && !self.closing && !self.stopped,
                                    operations_available: available,
                                    reduced: self.reduced,
                                },
                                &mut self.covers,
                                self.bridge.as_ref(),
                            );
                            if let Some(action) = action {
                                match action {
                                    crate::download_panel::QueueAction::Control(id, action) => {
                                        if let Some(bridge) = &self.bridge {
                                            match bridge.send(Command::Control(id, action)) {
                                                Ok(()) => {
                                                    self.controls.insert(id);
                                                }
                                                Err(error) => self.raise(error),
                                            }
                                        }
                                    }
                                    crate::download_panel::QueueAction::Operation(op) => {
                                        self.send(op)
                                    }
                                    crate::download_panel::QueueAction::Account(id) => {
                                        self.auth_target = Some(id);
                                        self.account_manager = true;
                                        self.send(Operation::Profiles);
                                    }
                                }
                            }
                        }
                        Page::Settings => {
                            if self.background.crop_open {
                                if self.background.show_crop(ui, &self.prefs) == CropEvent::Applied
                                {
                                    self.prefs.crop = self.background.draft();
                                    self.touch_prefs();
                                }
                            } else {
                                self.settings(ui);
                            }
                        }
                        Page::About => self.about(ui),
                    }
                });
            });
    }
}

fn normalize_face(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("http://") {
        format!("https://{rest}")
    } else if url.starts_with("//") {
        format!("https:{url}")
    } else {
        url.to_owned()
    }
}

fn circle_action(ui: &mut Ui, icon: Icon, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::click());
    ui.painter().circle_filled(
        rect.center(),
        18.0,
        if response.hovered() {
            theme::soft()
        } else {
            theme::card()
        },
    );
    ui.painter().circle_stroke(
        rect.center(),
        18.0,
        egui::Stroke::new(1.0_f32, theme::border()),
    );
    theme::icon(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::vec2(19.0, 19.0)),
        icon,
        theme::ink(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response.on_hover_text(label)
}

fn toast_close(ui: &mut Ui) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .circle_filled(rect.center(), 12.0, theme::soft());
    }
    theme::icon(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::vec2(12.0, 12.0)),
        Icon::Close,
        theme::muted_color(),
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn jump_to(ui: &mut Ui, section: usize, pending: &mut Option<usize>) {
    if *pending == Some(section) {
        ui.scroll_to_rect(
            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(1.0, 1.0)),
            Some(egui::Align::Min),
        );
        *pending = None;
    }
}

fn control_banner(error: &bili_core::CoreError) -> Banner {
    let mut banner = Banner::from(backend::error_message(error));
    banner.code = Some(backend::error_code(error));
    banner.verification = error.verification.clone();
    banner
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn checked_ago(epoch: u64) -> String {
    if epoch == 0 {
        return "从未检查".into();
    }
    let seconds = now_epoch().saturating_sub(epoch);
    if seconds < 60 {
        "刚刚".into()
    } else if seconds < 3600 {
        format!("{} 分钟前", seconds / 60)
    } else if seconds < 86_400 {
        format!("{} 小时前", seconds / 3600)
    } else {
        format!("{} 天前", seconds / 86_400)
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.collect(ctx);
        self.sync_appearance(ctx);
        if self.busy == Some(Busy::Search) {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        if self.updates.status().is_checking() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if self.error.is_some() || self.notice.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        if self.updates.poll() {
            match self.updates.status().clone() {
                UpdateStatus::Available(release) => {
                    self.prefs.last_check = now_epoch();
                    self.touch_prefs();
                    if self.prefs.update.auto_update {
                        self.updates.install();
                    } else if self.updates.should_notify() {
                        self.inform(Notice {
                            text: format!("发现新版本 {}", release.tag),
                            open_about: true,
                            open_downloads: false,
                        });
                    }
                }
                UpdateStatus::UpToDate | UpdateStatus::NoCompatibleRelease => {
                    self.prefs.last_check = now_epoch();
                    self.touch_prefs();
                }
                UpdateStatus::Ready { version } => {
                    self.inform(Notice {
                        text: format!("新版本 {version} 已安装，重启后生效。"),
                        open_about: true,
                        open_downloads: false,
                    });
                }
                UpdateStatus::Error(message) => self.raise(message),
                _ => {}
            }
        }
        if self.prefs.update.check_updates && !self.closing && self.updates.automatic_due() {
            self.updates.automatic(self.prefs.update_channel());
        }
        let maximized = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
        window::resize_edges(ctx, maximized);
        window::apply_rounded_corners(ctx, maximized);
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.prefs.confirm_close && !self.closing {
                self.confirming_close = true;
            } else {
                self.begin_close(ctx);
            }
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K))
            && !self.closing
            && !self.config.open
        {
            if self.page != Page::Discover {
                self.error = None;
                if self
                    .notice
                    .as_ref()
                    .is_some_and(|notice| !notice.open_about)
                {
                    self.notice = None;
                }
            }
            self.home();
            self.search.request_focus = true;
        }
        let screen = ctx.screen_rect();
        let clip = if self.prefs.background_scope == BackgroundScope::Full {
            screen
        } else {
            egui::Rect::from_min_max(egui::pos2(screen.left(), screen.top() + 92.0), screen.max)
        };
        self.background.paint(ctx, clip, &self.prefs);
        let bar_fill = self.surface(self.prefs.background_scope == BackgroundScope::Full);
        if let Some(action) = window::title_bar(ctx, bar_fill, maximized) {
            match action {
                WindowAction::Minimize => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true))
                }
                WindowAction::ToggleMaximize => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized))
                }
                WindowAction::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        self.header(ctx);
        self.central(ctx);
        self.configuration(ctx);
        self.accounts(ctx);
        self.verification_dialog(ctx, frame);
        self.toasts(ctx);
        let available = self.available();
        let login_running = self.busy == Some(Busy::Login);
        if !self.config.open {
            if let Some(action) = self.login.show(
                ctx,
                crate::login::LoginView {
                    auth: self.auth.as_ref(),
                    remembered: self.prefs.remember_login,
                    available,
                    running: login_running,
                    cancelling: self.cancelling,
                    reduced: self.reduced,
                },
            ) {
                match action {
                    LoginAction::Start { remember } => self.send(Operation::Login {
                        remember,
                        activate: self.auth_target.is_none(),
                    }),
                    LoginAction::Cancel => self.cancel(),
                    LoginAction::Logout => self.send(Operation::Logout),
                }
            }
        }
        if self.confirming_close {
            egui::Window::new("退出 Bili")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .frame(theme::card_frame())
                .show(ctx, |ui| {
                    theme::inset(theme::card()).show(ui, |ui| {
                        ui.label(RichText::new("要退出 Bili 吗？").size(20.0).strong());
                        theme::muted(ui, "正在进行的下载会暂停并保存断点，下次启动可以继续。");
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            if theme::button(
                                ui,
                                "继续使用",
                                Icon::Back,
                                ButtonKind::Quiet,
                                self.reduced,
                            )
                            .clicked()
                            {
                                self.confirming_close = false;
                            }
                            if theme::button(
                                ui,
                                "退出",
                                Icon::Close,
                                ButtonKind::Danger,
                                self.reduced,
                            )
                            .clicked()
                            {
                                self.confirming_close = false;
                                self.begin_close(ctx);
                            }
                        });
                    });
                });
        }
        if self.closing || self.close_error.is_some() {
            egui::Window::new("安全关闭")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .frame(theme::card_frame())
                .show(ctx, |ui| {
                    theme::inset(theme::card()).show(ui, |ui| {
                        if let Some(error) = &self.close_error {
                            ui.colored_label(theme::ERROR, error);
                            theme::muted(
                                ui,
                                "后台已停止，但未能确认所有状态已保存。请保留状态目录以便恢复。",
                            );
                            if theme::button(
                                ui,
                                "关闭窗口",
                                Icon::Close,
                                ButtonKind::Danger,
                                self.reduced,
                            )
                            .clicked()
                            {
                                self.allow_close = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        } else {
                            theme::loading(
                                ui,
                                self.reduced,
                                "正在暂停任务，保存断点并释放后台资源…",
                            );
                            theme::muted(
                                ui,
                                "正在进行的网络请求可能需要片刻结束，请不要强制退出。",
                            );
                        }
                    });
                });
        }
        self.save_prefs(ctx);
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use bili_core::{
        Aid, Bvid, Candidate, CandidatePage, CategoryPage, Cid, ContentRef, JobId, JobPhase,
        JobSnapshot, MediaItem, Part, PartRef, SearchCategory, SearchSort,
    };
    use egui::{Pos2, Rect, Vec2};

    /// Manual native inspection uses only synthetic in-memory data and the
    /// actual App painters. It never constructs Core, loads prefs or saves them.
    #[cfg(windows)]
    #[test]
    #[ignore = "opens a bounded offline native layout review window"]
    fn native_layout_review() {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        struct Review {
            app: App,
            category: SearchCategory,
            items: Vec<MediaItem>,
            started: Instant,
            lifetime: Duration,
            captures: Option<PathBuf>,
            settings_review: bool,
            scene: usize,
            scene_frames: usize,
            capture_requested: bool,
            captured: Arc<std::sync::atomic::AtomicUsize>,
            _root: tempfile::TempDir,
        }
        fn image(size: [usize; 2]) -> egui::ColorImage {
            let mut image = egui::ColorImage::new(size, egui::Color32::from_rgb(228, 231, 235));
            let w = size[0] as f32;
            let h = size[1] as f32;
            let small = w.min(h);
            let radius = small * 0.18;
            let center = if h > w {
                egui::pos2(w * 0.5, h * 0.25)
            } else {
                egui::pos2(w * 0.25, h * 0.5)
            };
            let side = small * 0.32;
            let square = if h > w {
                Rect::from_center_size(egui::pos2(w * 0.5, h * 0.73), Vec2::splat(side))
            } else {
                Rect::from_center_size(egui::pos2(w * 0.73, h * 0.5), Vec2::splat(side))
            };
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let p = egui::pos2(x as f32, y as f32);
                    if p.distance(center) <= radius {
                        image[(x, y)] = egui::Color32::from_rgb(80, 120, 190);
                    }
                    if square.contains(p) {
                        image[(x, y)] = egui::Color32::from_rgb(190, 90, 80);
                    }
                }
            }
            image
        }
        impl Review {
            fn scene_count(&self) -> usize {
                if self.settings_review {
                    12
                } else {
                    7
                }
            }

            fn capture_scene(&mut self) {
                if self.settings_review {
                    self.app.config = ConfigState::default();
                    self.app.search.input.clear();
                    self.load(SearchCategory::Video);
                    self.app.prefs.theme = match self.scene {
                        1 | 6 => "sunset",
                        2..=4 | 8 => "matcha",
                        _ => "violet",
                    }
                    .into();
                    self.app.prefs.opacity = match self.scene {
                        3 => 50.0,
                        4 => 0.0,
                        _ => 100.0,
                    };
                    match self.scene {
                        5 | 6 | 8 | 9 | 11 => {
                            if self.scene == 8 {
                                self.items.rotate_left(1);
                            }
                            self.configuration();
                            if self.scene == 6 {
                                self.app.config.review_rate_override(100.0);
                            } else if self.scene == 8 {
                                self.app.config.advanced_open = true;
                            }
                        }
                        7 => {
                            self.app.navigate(Page::Settings);
                            self.app.settings_jump = Some(2);
                        }
                        10 => self.app.detail = Some(DetailState::new(self.items[0].clone())),
                        _ => {}
                    }
                    return;
                }
                self.app.config.open = false;
                self.app.search.input.clear();
                self.load(match self.scene {
                    2 => SearchCategory::Film,
                    3 => SearchCategory::Video,
                    _ => SearchCategory::Bangumi,
                });
                match self.scene {
                    1 => self.app.search.input = "测试输入 ABC 123".into(),
                    4 => self.app.detail = Some(DetailState::new(self.items[0].clone())),
                    5 => self.configuration(),
                    6 => {
                        self.app.navigate(Page::Settings);
                        self.app.settings_jump = Some(2);
                    }
                    _ => {}
                }
            }

            fn load(&mut self, category: SearchCategory) {
                self.category = category;
                self.app.home();
                self.app.search.category = category;
                self.items = (0..4)
                    .map(|i| {
                        let mut item = media_item();
                        let id = Aid::new(1000 + i).unwrap();
                        let reference = if category == SearchCategory::Video {
                            ContentRef::Video(id)
                        } else {
                            ContentRef::Bangumi(bili_core::SeasonId::new(1000 + i).unwrap())
                        };
                        item.reference = reference.clone();
                        item.title = format!(
                            "离线{}封面 {}",
                            crate::search::category_name(category),
                            i + 1
                        );
                        item.cover = Some(
                            if category == SearchCategory::Video {
                                "fixture:landscape"
                            } else {
                                "fixture:portrait"
                            }
                            .into(),
                        );
                        item.parts[0].reference.content = reference.clone();
                        item.parts[0].reference.aid = id;
                        item.parts[0].reference.cid = Cid::new(2000 + i).unwrap();
                        item.parts[0].title = "第 1 项".into();
                        item.source = Some(bili_core::SourceProject {
                            reference,
                            title: item.title.clone(),
                            category: Some(category),
                            cover: item.cover.clone(),
                            ordinal: 1,
                        });
                        item.description = Some("仅用于检查图片比例和输入框的本地样例。".into());
                        item.author = Some("离线样例".into());
                        item
                    })
                    .collect();
                self.app.search.set_requested(SearchQuery {
                    keyword: "offline layout sample".into(),
                    category,
                    page: 1,
                    sort: bili_core::SearchSort::Relevance,
                    filter: Default::default(),
                });
                self.app.search.accept_results(CandidatePage {
                    pages: vec![CategoryPage {
                        category,
                        page: 1,
                        total: Some(96),
                        has_more: true,
                        page_size: Some(20),
                        total_pages: Some(5),
                        items: self
                            .items
                            .iter()
                            .map(|item| Candidate {
                                reference: item.reference.clone(),
                                category,
                                title: item.title.clone(),
                                cover: item.cover.clone(),
                                duration_ms: item.duration_ms,
                                author: item.author.clone(),
                                published_at: None,
                                plays: Some(12000),
                                danmaku: Some(100),
                            })
                            .collect(),
                    }],
                });
            }
            fn configuration(&mut self) {
                let item = self.items[0].clone();
                let part = item.parts[0].reference.clone();
                let mut catalog = bili_core::QualityCatalog {
                    part: part.clone(),
                    advertised: vec![bili_core::QualityEntry {
                        id: 64,
                        label: "720P".into(),
                        rank: 64,
                        availability: bili_core::Availability::Obtainable,
                        requires_login: None,
                        requires_vip: None,
                        has_video_url: true,
                        advertised_codecs: vec!["avc1".into()],
                    }],
                    representations: vec![bili_core::RepresentationKey {
                        part: part.clone(),
                        id: 64,
                        quality: 64,
                        codec: "avc1".into(),
                        width: Some(1280),
                        height: Some(720),
                        fps: None,
                        dynamic_range: None,
                        color: None,
                        bandwidth: 1000,
                        audio: false,
                        sample_rate: None,
                        channels: None,
                        language: None,
                    }],
                    revision: 1,
                    preview: false,
                };
                if self.settings_review {
                    let mut full_hd = catalog.representations[0].clone();
                    full_hd.id = 80;
                    full_hd.quality = 80;
                    full_hd.width = Some(1920);
                    full_hd.height = Some(1080);
                    catalog.representations.push(full_hd);
                    for (id, label, returned) in [(80, "1080P", true), (120, "4K", false)] {
                        catalog.advertised.push(bili_core::QualityEntry {
                            id,
                            label: label.into(),
                            rank: id,
                            availability: bili_core::Availability::Obtainable,
                            requires_login: None,
                            requires_vip: None,
                            has_video_url: returned,
                            advertised_codecs: vec!["avc1".into()],
                        });
                    }
                }
                let mut prepared = vec![crate::backend::PreparedItem {
                    item,
                    category: self.category,
                    selected: vec![part.clone()],
                    catalogs: vec![(part, Ok(catalog.clone()))],
                }];
                if self.settings_review && self.scene == 11 {
                    let item = self.items[1].clone();
                    let part = item.parts[0].reference.clone();
                    catalog.part = part.clone();
                    catalog.representations.retain(|r| r.quality == 64);
                    for representation in &mut catalog.representations {
                        representation.part = part.clone();
                    }
                    prepared.push(crate::backend::PreparedItem {
                        item,
                        category: self.category,
                        selected: vec![part.clone()],
                        catalogs: vec![(part, Ok(catalog))],
                    });
                }
                self.app.config.accept(
                    prepared,
                    None,
                    &self.app.output,
                    self.app.prefs.default_download_rate_mib,
                    false,
                );
            }
        }
        impl eframe::App for Review {
            fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
                if let Some(directory) = self.captures.clone() {
                    let screenshot = ctx.input(|input| {
                        input.events.iter().find_map(|event| {
                            if let egui::Event::Screenshot { image, .. } = event {
                                Some(image.clone())
                            } else {
                                None
                            }
                        })
                    });
                    if let Some(image) = screenshot {
                        if self.settings_review && self.scene <= 4 {
                            for (x, source) in [
                                (8, [100_u8, 180, 235, 255]),
                                (590, [255, 255, 255, 255]),
                                (1172, [16, 20, 30, 255]),
                            ] {
                                let pixel = image.pixels[(450 * image.size[1] / 800)
                                    * image.size[0]
                                    + x * image.size[0] / 1180]
                                    .to_array();
                                let expected = match self.scene {
                                    3 => source.map(|v| {
                                        ((f32::from(v) * 128.0 / 255.0) + 127.0).round() as u8
                                    }),
                                    4 => theme::background().to_array(),
                                    _ => source,
                                };
                                assert!(
                                    pixel.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 2),
                                    "native scene {} pixel x{x}: {pixel:?} != {expected:?}",
                                    self.scene
                                );
                                println!(
                                    "Native background scene {} pixel x{x}: {pixel:?}",
                                    self.scene
                                );
                            }
                        }
                        let pixels: Vec<u8> =
                            image.pixels.iter().flat_map(|p| p.to_array()).collect();
                        let file = directory.join(format!("{:02}.png", self.scene));
                        image::save_buffer(
                            &file,
                            &pixels,
                            image.size[0] as u32,
                            image.size[1] as u32,
                            image::ColorType::Rgba8,
                        )
                        .unwrap();
                        println!("Native rendered scene: {}", file.display());
                        self.captured
                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        self.scene += 1;
                        self.scene_frames = 0;
                        self.capture_requested = false;
                        if self.scene == self.scene_count() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            return;
                        }
                        self.capture_scene();
                        if self.settings_review && self.scene == 9 {
                            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                                900.0, 640.0,
                            )));
                        }
                    }
                }
                if self.started.elapsed() >= self.lifetime {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                self.app.sync_appearance(ctx);
                self.app
                    .background
                    .paint(ctx, ctx.screen_rect(), &self.app.prefs);
                egui::TopBottomPanel::bottom("offline-review-controls").show(ctx, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("离线检查：");
                        for (category, label) in [
                            (SearchCategory::Bangumi, "番剧样例"),
                            (SearchCategory::Film, "影视样例"),
                            (SearchCategory::Video, "视频样例"),
                        ] {
                            if ui.button(label).clicked() {
                                self.load(category);
                            }
                        }
                        if ui.button("详情样例").clicked() {
                            self.app.home();
                            self.app.detail = Some(DetailState::new(self.items[0].clone()));
                        }
                        if ui.button("下载目录样例").clicked() {
                            self.configuration();
                        }
                        if ui.button("设置样例").clicked() {
                            self.app.navigate(Page::Settings);
                            self.app.settings_jump = Some(2);
                        }
                    });
                });
                self.app.header(ctx);
                if self.app.search.category != self.category && self.app.page == Page::Discover {
                    self.load(self.app.search.category);
                }
                // Permit real cover clicks to enter the matching synthetic detail,
                // without a Bridge or an alternate production backend.
                match self.app.retry_resolve.take() {
                    Some(Retry::Resolve(input)) => {
                        if let Some(item) = self
                            .items
                            .iter()
                            .find(|item| crate::search::reference_input(&item.reference) == input)
                        {
                            self.app.detail = Some(DetailState::new(item.clone()));
                        }
                    }
                    Some(Retry::Prepare { .. }) => self.configuration(),
                    _ => {}
                }
                self.app.central(ctx);
                if let Some(operation) = self.app.config.show(ctx, self.app.available(), None) {
                    self.app.send(operation);
                }
                self.app.accounts(ctx);
                self.app.prefs_dirty = false;
                ctx.output_mut(|output| output.open_url = None);
                if self.captures.is_some() {
                    self.scene_frames += 1;
                    if self.scene_frames >= 4 && !self.capture_requested {
                        self.capture_requested = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
                    }
                }
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        let lifetime = std::env::var("BILI_GUI_LAYOUT_REVIEW_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(180)
            .clamp(5, 300);
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/native-gui-checks/layout-review");
        std::fs::create_dir_all(&base).unwrap();
        let root = tempfile::Builder::new()
            .prefix("isolated-")
            .tempdir_in(&base)
            .unwrap();
        assert!(root
            .path()
            .canonicalize()
            .unwrap()
            .starts_with(base.canonicalize().unwrap()));
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("BiliDL offline layout review")
                .with_inner_size([1180.0, 800.0])
                .with_min_inner_size([900.0, 640.0])
                .with_active(false),
            persist_window: false,
            persistence_path: Some(root.path().join("eframe")),
            event_loop_builder: Some(Box::new(|builder| {
                builder.with_any_thread(true);
            })),
            ..Default::default()
        };
        let captures = std::env::var_os("BILI_GUI_LAYOUT_CAPTURE").map(|_| {
            let directory = base.join("captures").join(root.path().file_name().unwrap());
            std::fs::create_dir_all(&directory).unwrap();
            directory
        });
        let capture_enabled = captures.is_some();
        let settings_review = std::env::var_os("BILI_GUI_SETTINGS_REVIEW").is_some();
        let captured = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = captured.clone();
        eframe::run_native(
            "BiliDL offline layout review",
            options,
            Box::new(move |cc| {
                theme::install(&cc.egui_ctx, theme::system_fonts());
                let mut app = App::new(
                    None,
                    root.path().join("downloads"),
                    root.path().join("state"),
                    false,
                    None,
                    GuiPrefs::default(),
                );
                app.ready = true;
                if settings_review {
                    let path = root.path().join("background.png");
                    image::RgbaImage::from_fn(1180, 800, |x, _| {
                        image::Rgba(match x {
                            0..=392 => [100, 180, 235, 255],
                            393..=786 => [255, 255, 255, 255],
                            _ => [16, 20, 30, 255],
                        })
                    })
                    .save(&path)
                    .unwrap();
                    app.prefs.background = Some(path);
                    app.prefs.default_download_rate_mib = 10.0;
                }
                app.covers.insert(
                    &cc.egui_ctx,
                    "fixture:portrait".into(),
                    Some(image([300, 420])),
                );
                app.covers.insert(
                    &cc.egui_ctx,
                    "fixture:landscape".into(),
                    Some(image([420, 236])),
                );
                let mut review = Review {
                    app,
                    category: SearchCategory::Bangumi,
                    items: vec![],
                    started: Instant::now(),
                    lifetime: Duration::from_secs(lifetime),
                    captures,
                    settings_review,
                    scene: 0,
                    scene_frames: 0,
                    capture_requested: false,
                    captured,
                    _root: root,
                };
                review.capture_scene();
                Ok(Box::new(review))
            }),
        )
        .unwrap();
        if capture_enabled {
            assert_eq!(
                count.load(std::sync::atomic::Ordering::SeqCst),
                if settings_review { 12 } else { 7 },
                "native renderer must return every requested scene"
            );
        }
    }

    fn test_app() -> App {
        let prefs = GuiPrefs::default();
        let mut app = App::new(
            None,
            std::env::temp_dir().join("bili-gui-layout-output"),
            std::env::temp_dir().join("bili-gui-layout-state"),
            false,
            None,
            prefs,
        );
        app.ready = true;
        app.initializing = false;
        app
    }

    #[test]
    fn resetting_appearance_keeps_the_download_rate_default() {
        let mut app = test_app();
        app.prefs.default_download_rate_mib = 10.0;
        app.prefs.theme = "matcha".into();
        app.reset_appearance();
        assert_eq!(app.prefs.default_download_rate_mib, 10.0);
        assert_eq!(app.prefs.theme, GuiPrefs::default().theme);
    }

    #[test]
    fn download_dialog_blocks_background_clicks_and_prior_keyboard_focus() {
        let size = Vec2::new(1180.0, 800.0);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.page = Page::Queue;
        app.search.input = "unchanged search".into();
        click_text(&mut ctx, &mut app, size, "unchanged search");
        app.config.open = true;
        frame(&mut ctx, &mut app, size, vec![]);
        frame(
            &mut ctx,
            &mut app,
            size,
            vec![egui::Event::Text("blocked".into())],
        );
        assert_eq!(app.search.input, "unchanged search");
        click_text(&mut ctx, &mut app, size, "BiliDL");
        assert_eq!(app.page, Page::Queue);
        assert!(app.config.open);
        frame(
            &mut ctx,
            &mut app,
            size,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert!(!app.config.open);
        click_text(&mut ctx, &mut app, size, "BiliDL");
        assert_eq!(app.page, Page::Discover);
    }

    fn frame(
        ctx: &mut egui::Context,
        app: &mut App,
        size: Vec2,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        ctx.run(raw, |ctx| {
            app.header(ctx);
            app.central(ctx);
            app.configuration(ctx);
            app.accounts(ctx);
            app.toasts(ctx);
        })
    }

    fn texts(output: &egui::FullOutput) -> Vec<(String, Rect, Rect)> {
        fn walk(shape: &egui::Shape, clip: Rect, out: &mut Vec<(String, Rect, Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                    out.push((text.galley.job.text.clone(), rect, clip));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, clip, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, clipped.clip_rect, &mut out);
        }
        out
    }

    fn assert_visible(output: &egui::FullOutput, needle: &str, screen: Rect) {
        let found: Vec<_> = texts(output)
            .into_iter()
            .filter(|(text, _, _)| text.contains(needle))
            .collect();
        assert!(!found.is_empty(), "missing text: {needle}");
        for (text, rect, clip) in found {
            assert!(
                screen.expand(1.0).contains_rect(rect),
                "{text} is outside the viewport: rect {rect:?}"
            );
            assert!(
                clip.expand(1.0).contains_rect(rect),
                "{text} is clipped: rect {rect:?} clip {clip:?}"
            );
        }
    }
    fn click_text(ctx: &mut egui::Context, app: &mut App, size: Vec2, needle: &str) {
        frame(ctx, app, size, vec![]);
        let output = frame(ctx, app, size, vec![]);
        let pos = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text == needle)
            .unwrap_or_else(|| panic!("missing control {needle}"))
            .1
            .center();
        frame(
            ctx,
            app,
            size,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        frame(
            ctx,
            app,
            size,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
    }

    #[test]
    fn search_detail_shared_configuration_admission_and_history_form_one_flow() {
        use bili_core::{Availability, QualityCatalog, QualityEntry, RepresentationKey};
        let size = Vec2::new(1180.0, 800.0);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.prefs.default_download_rate_mib = 10.0;
        let (bridge, events, mut commands) = Bridge::test_channel();
        app.bridge = Some(bridge);
        let item = media_item();
        let part = item.parts[0].reference.clone();
        app.search.set_requested(SearchQuery {
            keyword: "视频".into(),
            category: SearchCategory::Video,
            page: 1,
            sort: bili_core::SearchSort::Relevance,
            filter: Default::default(),
        });
        app.search.accept_results(CandidatePage {
            pages: vec![CategoryPage {
                category: SearchCategory::Video,
                page: 1,
                total: Some(1),
                has_more: false,
                page_size: Some(20),
                total_pages: Some(1),
                items: vec![Candidate {
                    reference: item.reference.clone(),
                    category: SearchCategory::Video,
                    title: item.title.clone(),
                    cover: None,
                    duration_ms: item.duration_ms,
                    author: None,
                    published_at: None,
                    plays: None,
                    danmaku: None,
                }],
            }],
        });
        click_text(&mut ctx, &mut app, size, &item.title);
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::Run(Operation::Resolve(_)))
        ));
        assert!(events
            .try_send(Event::Finished(Ok(Outcome::Detail(item.clone()))))
            .is_ok());
        app.collect(&ctx);
        assert!(app.detail.is_some());
        click_text(&mut ctx, &mut app, size, "下载");
        let Ok(Command::Run(Operation::Prepare { inputs, .. })) = commands.try_recv() else {
            panic!("detail must use shared preparation")
        };
        assert_eq!(inputs[0].parts.as_ref().unwrap(), &vec![part.clone()]);
        let video = RepresentationKey {
            part: part.clone(),
            id: 64,
            quality: 64,
            codec: "avc1".into(),
            width: Some(1280),
            height: Some(720),
            fps: None,
            dynamic_range: None,
            color: None,
            bandwidth: 1000,
            audio: false,
            sample_rate: None,
            channels: None,
            language: None,
        };
        let catalog = QualityCatalog {
            part: part.clone(),
            advertised: vec![QualityEntry {
                id: 64,
                label: "720P".into(),
                rank: 64,
                availability: Availability::Obtainable,
                requires_login: None,
                requires_vip: None,
                has_video_url: true,
                advertised_codecs: vec!["avc1".into()],
            }],
            representations: vec![video.clone()],
            revision: 1,
            preview: false,
        };
        assert!(events
            .try_send(Event::Finished(Ok(Outcome::Prepared {
                refresh: false,
                items: vec![crate::backend::PreparedItem {
                    item,
                    category: SearchCategory::Video,
                    selected: vec![part.clone()],
                    catalogs: vec![(part.clone(), Ok(catalog))]
                }],
                profile: None
            })))
            .is_ok());
        app.collect(&ctx);
        assert!(app.config.open);
        click_text(&mut ctx, &mut app, size, "开始下载 · 1 项");
        let Ok(Command::Run(Operation::Enqueue {
            selections,
            output_dir,
        })) = commands.try_recv()
        else {
            panic!("configured admission")
        };
        assert_eq!(selections.len(), 1);
        assert_eq!(selections[0].selection.part, part);
        assert_eq!(
            selections[0].context.batch.as_ref().unwrap().rate_limit,
            Some(10 * 1_048_576),
            "new download preparation inherits the persisted GUI default"
        );
        let request = &selections[0];
        let plan = bili_core::StreamPlan {
            schema_version: 1,
            fingerprint: "synthetic-native-flow".into(),
            part: part.clone(),
            title: request.selection.title.clone(),
            video,
            audio: None,
            topology: bili_core::StreamTopology::Dash,
            segments: vec![],
            destination: output_dir.join("native-flow.mp4"),
            merge: bili_core::MergePolicy::Automatic,
            sidecars: request.selection.sidecars.clone(),
            keep_temp: false,
            catalog_revision: 1,
            context: Some(request.context.clone()),
        };
        let id = JobId::new();
        assert!(events
            .try_send(Event::Finished(Ok(Outcome::Enqueued {
                receipts: vec![crate::backend::AdmissionReceipt {
                    id,
                    plan: plan.clone()
                }],
                cancelled: false,
                error: None
            })))
            .is_ok());
        app.collect(&ctx);
        assert_eq!(app.page, Page::Discover);
        assert!(app.config.open);
        assert_eq!(app.config.receipts[0].id, id);
        app.config.open = false;
        app.navigate(Page::Queue);
        let mut completed = job(JobPhase::Completed, Some("C:/synthetic/native-flow.mp4"));
        completed.id = id;
        completed.plan = Some(Arc::new(plan));
        app.snapshot = Arc::new(QueueSnapshot {
            schema_version: 1,
            revision: 1,
            jobs: vec![Arc::new(completed)],
        });
        click_text(&mut ctx, &mut app, size, "已完成 1");
        let output = frame(&mut ctx, &mut app, size, vec![]);
        assert_visible(
            &output,
            "打开文件所在位置",
            Rect::from_min_size(Pos2::ZERO, size),
        );
    }

    fn results(count: usize) -> CandidatePage {
        CandidatePage {
            pages: vec![CategoryPage {
                category: SearchCategory::Video,
                page: 1,
                total: Some(count as u64),
                has_more: false,
                page_size: Some(count as u32),
                total_pages: Some(1),
                items: (0..count)
                    .map(|index| Candidate {
                        reference: ContentRef::Video(Aid::new(1_000 + index as u64).unwrap()),
                        category: SearchCategory::Video,
                        title: format!("测试视频 {index}"),
                        cover: None,
                        duration_ms: Some(61_000),
                        author: None,
                        published_at: None,
                        plays: None,
                        danmaku: None,
                    })
                    .collect(),
            }],
        }
    }

    fn media_item() -> MediaItem {
        let aid = Aid::new(80_433_022).unwrap();
        let cid = Cid::new(137_649_199).unwrap();
        MediaItem {
            reference: ContentRef::Video(aid),
            bvid: Some(Bvid::new("BV1GJ411x7h7").unwrap()),
            title: "【官方 MV】Never Gonna Give You Up - Rick Astley".into(),
            cover: None,
            duration_ms: Some(213_000),
            source: None,
            description: None,
            author: None,
            parts: vec![Part {
                reference: PartRef {
                    content: ContentRef::Video(aid),
                    aid,
                    cid,
                    episode: None,
                },
                ordinal: 1,
                title: "Never Gonna Give You Up - Rick Astley".into(),
                duration_ms: Some(213_000),
                preview: false,
            }],
        }
    }

    fn job(phase: JobPhase, output: Option<&str>) -> JobSnapshot {
        JobSnapshot {
            id: JobId::new(),
            revision: 1,
            plan: None,
            phase,
            downloaded: 0,
            total: None,
            output: output.map(PathBuf::from),
            skipped: false,
            error: None,
            components: Vec::new(),
            created_at: None,
        }
    }

    fn wheel(events: &mut Vec<egui::Event>, at: Pos2, notches: i32) {
        events.push(egui::Event::PointerMoved(at));
        for _ in 0..notches {
            events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -60.0),
                modifiers: egui::Modifiers::default(),
            });
        }
    }

    #[test]
    fn detail_navigation_starts_at_the_top_and_keeps_search_scroll() {
        let size = Vec2::new(1180.0, 800.0);
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        // The grid fits twelve short cards almost entirely in this viewport.
        // Use six rows so the wheel exercise measures scrolling rather than
        // clamping against a few pixels of overflow.
        app.search.results = Some(results(24));

        let output = frame(&mut ctx, &mut app, size, Vec::new());
        let before = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text == "测试视频 0")
            .map(|(_, rect, _)| rect)
            .expect("first result card title");

        let mut events = Vec::new();
        wheel(&mut events, egui::pos2(600.0, 500.0), 3);
        frame(&mut ctx, &mut app, size, events);
        // egui spreads wheel input across animation frames. Let that input
        // finish before comparing offsets across separate workspaces.
        let mut output = frame(&mut ctx, &mut app, size, Vec::new());
        for _ in 0..30 {
            output = frame(&mut ctx, &mut app, size, Vec::new());
        }
        let after = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text == "测试视频 0")
            .map(|(_, rect, _)| rect)
            .expect("first result title after scrolling");
        assert!(
            after.top() < before.top() - 50.0,
            "search results did not scroll: before {before:?} after {after:?}"
        );

        app.detail = Some(DetailState::new(media_item()));
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert_visible(&output, "返回搜索", screen);

        click_text(&mut ctx, &mut app, size, "返回搜索");
        assert!(app.detail.is_none());
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        let restored = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text == "测试视频 0")
            .map(|(_, rect, _)| rect)
            .expect("first result title after going back");
        assert!(
            (restored.top() - after.top()).abs() < 1.0,
            "search scroll was not preserved: before {after:?} restored {restored:?}"
        );
    }

    #[test]
    fn ui_scale_waits_for_explicit_apply() {
        let size = Vec2::new(1180.0, 800.0);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.page = Page::Settings;
        app.settings_jump = Some(3);

        frame(&mut ctx, &mut app, size, Vec::new());
        app.scale_draft = 1.25;
        frame(&mut ctx, &mut app, size, Vec::new());
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert!(
            texts(&output).iter().any(|(text, _, _)| text == "应用缩放"),
            "pending ui scale must offer an explicit apply button"
        );
        assert!(
            (app.prefs.ui_scale - 1.0).abs() < f32::EPSILON,
            "dragging the slider applied the scale immediately: {}",
            app.prefs.ui_scale
        );
    }

    #[test]
    fn update_button_is_centered_in_the_about_card() {
        let size = Vec2::new(1180.0, 800.0);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.page = Page::About;

        frame(&mut ctx, &mut app, size, Vec::new());
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        let title = texts(&output)
            .into_iter()
            .find(|(text, rect, _)| text == "Bili" && rect.center().x > 300.0)
            .map(|(_, rect, _)| rect)
            .expect("about title");
        let button = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text == "检查更新")
            .map(|(_, rect, _)| rect)
            .expect("check-update button");
        assert!(
            (button.center().x - title.center().x).abs() < 16.0,
            "check-update button is not centered: title {title:?} button {button:?}"
        );
    }

    #[test]
    fn banners_stay_visible_while_content_scrolls() {
        let size = Vec2::new(1180.0, 800.0);
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.search.results = Some(results(12));
        app.error = Some("布局测试横幅".into());

        frame(&mut ctx, &mut app, size, Vec::new());
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert_visible(&output, "布局测试横幅", screen);
        let toast = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text == "布局测试横幅")
            .map(|(_, rect, _)| rect)
            .unwrap();
        assert!(
            toast.left() > size.x * 0.6,
            "toast is not anchored top-right: {toast:?}"
        );

        let mut events = Vec::new();
        wheel(&mut events, egui::pos2(600.0, 500.0), 20);
        frame(&mut ctx, &mut app, size, events);
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert_visible(&output, "布局测试横幅", screen);
    }

    #[test]
    fn busy_status_floats_top_right_instead_of_inline() {
        let size = Vec2::new(1180.0, 800.0);
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.search.results = Some(results(12));
        app.busy = Some(Busy::Resolve);

        frame(&mut ctx, &mut app, size, Vec::new());
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert_visible(&output, "正在解析内容与分集…", screen);
        let status = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text == "正在解析内容与分集…")
            .map(|(_, rect, _)| rect)
            .unwrap();
        assert!(
            status.left() > size.x * 0.6,
            "busy status is not floating top-right: {status:?}"
        );
        let cancel = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text == "取消操作")
            .map(|(_, rect, _)| rect)
            .expect("cancel button for a cancellable operation");
        assert!(
            cancel.left() > size.x * 0.6,
            "cancel button is not part of the floating card: {cancel:?}"
        );

        app.busy = None;
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert!(
            !texts(&output)
                .iter()
                .any(|(text, _, _)| text == "正在解析内容与分集…"),
            "busy status lingered after the operation finished"
        );
    }

    #[test]
    fn pages_stay_inside_the_viewport_at_common_sizes() {
        for size in [
            Vec2::new(1180.0, 800.0),
            Vec2::new(900.0, 640.0),
            Vec2::new(1706.0, 960.0),
        ] {
            for page in [Page::Settings, Page::About] {
                let mut ctx = egui::Context::default();
                let mut app = test_app();
                app.page = page;
                let output = frame(&mut ctx, &mut app, size, Vec::new());
                for (text, rect, _) in texts(&output) {
                    assert!(
                        rect.left() >= -1.0 && rect.right() <= size.x + 1.0,
                        "{page:?} text {text} overflows at {size:?}: {rect:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn completed_and_active_rows_keep_their_controls_visible() {
        let size = Vec2::new(1180.0, 800.0);
        let screen = Rect::from_min_size(Pos2::ZERO, size);

        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.page = Page::Queue;
        app.queue
            .select_filter(crate::download_panel::Filter::Completed);
        app.snapshot = Arc::new(QueueSnapshot {
            schema_version: 1,
            revision: 1,
            jobs: vec![Arc::new(job(
                JobPhase::Completed,
                Some(r"C:\Users\Public\bili-test.mp4"),
            ))],
        });
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert_visible(&output, "打开文件所在位置", screen);

        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.page = Page::Queue;
        app.snapshot = Arc::new(QueueSnapshot {
            schema_version: 1,
            revision: 1,
            jobs: vec![Arc::new(job(JobPhase::Downloading, None))],
        });
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert_visible(&output, "暂停", screen);
    }

    fn header_circles(output: &egui::FullOutput) -> Vec<egui::Rect> {
        fn visit(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Circle(circle) if circle.radius >= 17.0 && circle.radius <= 18.5 => {
                    out.push(egui::Rect::from_center_size(
                        circle.center,
                        Vec2::splat(circle.radius * 2.0),
                    ))
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        visit(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut result = Vec::new();
        for shape in &output.shapes {
            visit(&shape.shape, &mut result);
        }
        result.sort_by(|a, b| a.left().total_cmp(&b.left()));
        result.dedup();
        result
    }
    #[test]
    fn header_has_logo_and_right_edge_circular_actions() {
        for size in [
            Vec2::new(900.0, 640.0),
            Vec2::new(1180.0, 800.0),
            Vec2::new(1706.0, 960.0),
        ] {
            let mut ctx = egui::Context::default();
            let mut app = test_app();
            let output = frame(&mut ctx, &mut app, size, vec![]);
            assert_visible(&output, "BiliDL", Rect::from_min_size(Pos2::ZERO, size));
            let circles = header_circles(&output);
            assert_eq!(circles.len(), 2);
            assert!((size.x - circles[1].right() - 18.0).abs() < 2.0);
            assert!((circles[0].width() - circles[1].width()).abs() < 0.1);
            assert!(!texts(&output).iter().any(
                |(text, _, _)| text.contains("你的工作空间") || text.contains("NATIVE DESKTOP")
            ));
        }
    }
    #[test]
    fn header_avatar_opens_menu_and_history_changes_workspace() {
        let size = Vec2::new(1180.0, 800.0);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        let output = frame(&mut ctx, &mut app, size, vec![]);
        let circles = header_circles(&output);
        for (pos, avatar) in [(circles[0].center(), true), (circles[1].center(), false)] {
            frame(
                &mut ctx,
                &mut app,
                size,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                ],
            );
            frame(
                &mut ctx,
                &mut app,
                size,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            );
            if avatar {
                assert!(app.account_menu);
            } else {
                assert_eq!(app.page, Page::Queue);
            }
        }
    }

    #[test]
    fn expired_toast_disappears_automatically() {
        let size = Vec2::new(1180.0, 800.0);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.error = Some("过期的布局测试横幅".into());
        app.error_at = Some(Instant::now() - Duration::from_secs(30));
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert!(app.error.is_none(), "expired error was not cleared");
        assert!(
            !texts(&output)
                .iter()
                .any(|(text, _, _)| text.contains("过期的布局测试横幅")),
            "expired toast is still painted"
        );
    }

    #[test]
    fn explicit_home_invalidates_pending_detail_and_preparation_navigation() {
        let mut app = test_app();
        let (bridge, events, mut commands) = Bridge::test_channel();
        app.bridge = Some(bridge);
        let ctx = egui::Context::default();
        let item = media_item();
        let reference = item.reference.clone();
        app.send(Operation::Resolve("av80433022".into()));
        assert!(commands.try_recv().is_ok());
        app.home();
        assert!(events
            .try_send(Event::Finished(Ok(Outcome::Detail(item.clone()))))
            .is_ok());
        app.collect(&ctx);
        assert_eq!(app.page, Page::Discover);
        assert!(app.detail.is_none());
        assert!(app.detail_drafts.contains_key(&reference));
        let selected = item.parts.iter().map(|p| p.reference.clone()).collect();
        let prepared = crate::backend::PreparedItem {
            item,
            category: SearchCategory::Video,
            selected,
            catalogs: vec![],
        };
        app.send(Operation::Prepare {
            refresh: false,
            inputs: vec![],
            profile: None,
        });
        assert!(commands.try_recv().is_ok());
        app.home();
        assert!(events
            .try_send(Event::Finished(Ok(Outcome::Prepared {
                refresh: false,
                items: vec![prepared.clone()],
                profile: None
            })))
            .is_ok());
        app.collect(&ctx);
        assert!(
            !app.config.open,
            "a late response must not reopen a dismissed workflow"
        );
        let temporary = std::env::temp_dir().join("bili-native-cancelled-draft");
        app.config.directory = temporary.display().to_string();
        app.send(Operation::Prepare {
            refresh: false,
            inputs: vec![],
            profile: None,
        });
        assert!(commands.try_recv().is_ok());
        assert!(events
            .try_send(Event::Finished(Ok(Outcome::Prepared {
                refresh: false,
                items: vec![prepared],
                profile: None
            })))
            .is_ok());
        app.collect(&ctx);
        assert!(app.config.open);
        assert_eq!(PathBuf::from(&app.config.directory), temporary);
    }

    #[test]
    fn cancelled_read_success_does_not_open_a_flow_or_replace_committed_results() {
        let mut app = test_app();
        let (bridge, events, mut commands) = Bridge::test_channel();
        app.bridge = Some(bridge);
        let ctx = egui::Context::default();
        app.send(Operation::Resolve("av80433022".into()));
        let _ = commands.try_recv().unwrap();
        app.cancel();
        assert!(matches!(commands.try_recv().unwrap(), Command::Cancel));
        assert!(events
            .try_send(Event::Finished(Ok(Outcome::Detail(media_item()))))
            .is_ok());
        app.collect(&ctx);
        assert!(app.detail.is_none());

        let old = SearchQuery {
            keyword: "committed".into(),
            category: SearchCategory::Video,
            page: 1,
            sort: SearchSort::Relevance,
            filter: Default::default(),
        };
        app.search.set_requested(old);
        app.search.accept_results(results(2));
        let next = SearchQuery {
            keyword: "cancelled".into(),
            category: SearchCategory::Video,
            page: 1,
            sort: SearchSort::Relevance,
            filter: Default::default(),
        };
        app.search.set_requested(next.clone());
        app.send(Operation::Search(next));
        let _ = commands.try_recv().unwrap();
        app.cancel();
        let _ = commands.try_recv().unwrap();
        assert!(events
            .try_send(Event::Finished(Ok(Outcome::Search(results(0)))))
            .is_ok());
        app.collect(&ctx);
        assert_eq!(app.search.results.as_ref().unwrap().pages[0].items.len(), 2);
        assert!(app.search.stale);
        assert!(app.search.requested_query().is_none());
    }

    #[test]
    fn closing_configuration_during_refresh_keeps_it_closed_after_success() {
        let mut app = test_app();
        let (bridge, events, mut commands) = Bridge::test_channel();
        app.bridge = Some(bridge);
        let mut ctx = egui::Context::default();
        let item = media_item();
        let prepared = crate::backend::PreparedItem {
            selected: item.parts.iter().map(|p| p.reference.clone()).collect(),
            item,
            category: SearchCategory::Video,
            catalogs: vec![],
        };
        app.config.accept(
            vec![prepared.clone()],
            None,
            &app.output,
            app.prefs.default_download_rate_mib,
            false,
        );
        app.config.directory = std::env::temp_dir()
            .join("preserved-refresh-draft")
            .display()
            .to_string();
        app.send(Operation::Prepare {
            inputs: vec![],
            profile: None,
            refresh: true,
        });
        let _ = commands.try_recv().unwrap();
        frame(
            &mut ctx,
            &mut app,
            Vec2::new(1180.0, 800.0),
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!app.config.open);
        assert!(events
            .try_send(Event::Finished(Ok(Outcome::Prepared {
                items: vec![prepared],
                profile: None,
                refresh: true
            })))
            .is_ok());
        app.collect(&ctx);
        assert!(
            !app.config.open,
            "late catalog refresh must respect dialog dismissal"
        );
        assert_eq!(
            PathBuf::from(&app.config.directory),
            std::env::temp_dir().join("preserved-refresh-draft")
        );
    }

    #[test]
    fn failed_category_and_preparation_retry_the_same_typed_intent() {
        let mut app = test_app();
        let (bridge, events, mut commands) = Bridge::test_channel();
        app.bridge = Some(bridge);
        let ctx = egui::Context::default();
        let section = bili_core::PgcSection::Film;
        let operation = app.search.choose_category(SearchCategory::Film).unwrap();
        app.send(operation);
        let _ = commands.try_recv().unwrap();
        assert!(events
            .try_send(Event::Finished(Err(bili_core::CoreError::new(
                ErrorCode::Transport,
                "test category failure"
            ))))
            .is_ok());
        app.collect(&ctx);
        let retry = app.error.as_ref().unwrap().retry.clone().unwrap();
        assert!(matches!(
            retry,
            Retry::BrowseConditions(bili_core::PgcSection::Film)
        ));
        app.run_retry(retry);
        assert!(
            matches!(commands.try_recv().unwrap(),Command::Run(Operation::BrowseConditions(actual)) if actual==section)
        );
        // Finish the retry before testing an independent prepared-batch failure.
        assert!(events
            .try_send(Event::Finished(Err(bili_core::CoreError::new(
                ErrorCode::Transport,
                "test category failure"
            ))))
            .is_ok());
        app.collect(&ctx);
        let item = media_item();
        let expected = item.parts[0].reference.clone();
        app.send(Operation::Prepare {
            inputs: vec![backend::PrepareInput {
                input: "av80433022".into(),
                parts: Some(vec![expected.clone()]),
                category: SearchCategory::Video,
            }],
            profile: Some("batch-profile".into()),
            refresh: true,
        });
        let _ = commands.try_recv().unwrap();
        assert!(events
            .try_send(Event::Finished(Err(bili_core::CoreError::new(
                ErrorCode::Transport,
                "test preparation failure"
            ))))
            .is_ok());
        app.collect(&ctx);
        app.run_retry(app.error.as_ref().unwrap().retry.clone().unwrap());
        let Command::Run(Operation::Prepare {
            inputs,
            profile,
            refresh,
        }) = commands.try_recv().unwrap()
        else {
            panic!("retry must preserve preparation rather than resolve a prior detail")
        };
        assert_eq!(profile.as_deref(), Some("batch-profile"));
        assert!(refresh);
        assert_eq!(
            inputs[0].parts.as_deref(),
            Some(std::slice::from_ref(&expected))
        );
    }

    #[test]
    fn adding_a_task_account_preserves_global_selection_and_retargets_only_that_task() {
        use bili_core::auth::{AuthAccount, AuthProfile, ProfileValidity};
        for global in [None, Some("global-profile")] {
            let mut app = test_app();
            let (bridge, events, mut commands) = Bridge::test_channel();
            app.bridge = Some(bridge);
            let ctx = egui::Context::default();
            let target = JobId::new();
            let added = AuthProfile {
                id: "new-task-profile".into(),
                account: AuthAccount {
                    mid: 22,
                    name: "task account".into(),
                    face: None,
                    vip: None,
                },
                remembered: false,
                validity: ProfileValidity::Valid,
                checked_at: None,
                expires_at: None,
            };
            let auth = AuthSnapshot {
                logged_in: global.is_some(),
                account: global.map(|_| AuthAccount {
                    mid: 11,
                    name: "global account".into(),
                    face: None,
                    vip: None,
                }),
                notice: None,
                qr: None,
                qr_status: None,
                profiles: vec![added],
                selected_profile: global.map(str::to_owned),
                login_profile: None,
            };
            app.auth = Some(auth.clone());
            app.auth_target = Some(target);
            app.account_manager = true;
            app.login.open = true;
            app.login.adding = true;
            let original_remember = app.prefs.remember_login;
            app.send(Operation::Login {
                remember: true,
                activate: false,
            });
            assert!(matches!(
                commands.try_recv().unwrap(),
                Command::Run(Operation::Login {
                    activate: false,
                    ..
                })
            ));
            assert!(events
                .try_send(Event::Finished(Ok(Outcome::ProfileAdded {
                    auth,
                    profile_id: "new-task-profile".into()
                })))
                .is_ok());
            app.collect(&ctx);
            assert_eq!(
                app.auth.as_ref().unwrap().selected_profile.as_deref(),
                global
            );
            assert_eq!(
                app.prefs.remember_login, original_remember,
                "task credentials must not change global auto-login preference"
            );
            assert!(!app.login.open);
            assert!(!app.account_manager);
            let Command::Run(Operation::RetargetProfile { id, profile }) =
                commands.try_recv().unwrap()
            else {
                panic!("added task account must retarget its original job")
            };
            assert_eq!(id, target);
            assert_eq!(profile.as_deref(), Some("new-task-profile"));
        }
    }

    #[test]
    fn error_banner_offers_retry_and_keeps_the_code_out_of_the_message() {
        let size = Vec2::new(1180.0, 800.0);
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.error = Some(Banner {
            message: "搜索没有完成".into(),
            hint: Some("可切换到「视频」后重试。".into()),
            code: Some("错误码 Transport".into()),
            retry: Some(Retry::Login),
            verification: None,
        });
        frame(&mut ctx, &mut app, size, Vec::new());
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert_visible(&output, "搜索没有完成", screen);
        assert_visible(&output, "重试", screen);
        assert_visible(&output, "可切换到「视频」后重试。", screen);
        assert!(
            !texts(&output)
                .iter()
                .any(|(text, _, _)| text.contains("错误码 Transport")),
            "the code belongs in a tooltip, not the toast body"
        );
        let message = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text == "搜索没有完成")
            .map(|(_, rect, _)| rect)
            .unwrap();
        assert!(message.left() > size.x * 0.6, "toast is not top-right");
    }

    #[test]
    fn update_notice_is_not_duplicated_on_the_about_page() {
        let size = Vec2::new(1180.0, 800.0);
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.notice = Some(Notice {
            text: "发现新版本 v9.9.9".into(),
            open_about: true,
            open_downloads: false,
        });
        frame(&mut ctx, &mut app, size, Vec::new());
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert_visible(&output, "发现新版本 v9.9.9", screen);

        app.page = Page::About;
        frame(&mut ctx, &mut app, size, Vec::new());
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert!(
            !texts(&output)
                .iter()
                .any(|(text, _, _)| text.contains("发现新版本 v9.9.9")),
            "the About card must be the only update notice"
        );
    }

    #[test]
    fn wide_about_page_uses_two_columns() {
        let size = Vec2::new(1706.0, 960.0);
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.page = Page::About;
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert_visible(&output, "检查更新", screen);
        assert_visible(&output, "项目与支持", screen);
    }
}
