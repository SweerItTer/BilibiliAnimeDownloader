//! Native application state. Paint only reads snapshots and sends bounded commands.
use crate::{
    backend::{self, Bridge, Command, Event, Operation, Outcome},
    background::{Background, CropEvent},
    covers::CoverCache,
    detail::DetailState,
    download_panel::QueueState,
    login::{LoginAction, LoginState},
    prefs::{BackgroundScope, GuiPrefs, StartPage, UpdateChannel, THEMES},
    search::SearchState,
    theme::{self, ButtonKind, Icon},
    update::{self, Status as UpdateStatus, UpdateState},
    window::{self, Action as WindowAction},
};
use bili_core::{AuthSnapshot, ContentRef, ErrorCode, JobId, JobPhase, QueueSnapshot, SearchQuery};
use egui::{RichText, Ui};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SIDEBAR_WIDTH: f32 = 210.0;
const CONTENT_MAX_WIDTH: f32 = 1120.0;
const TOAST_WIDTH: f32 = 380.0;
const TOAST_ERROR_TTL: Duration = Duration::from_secs(12);
const TOAST_NOTICE_TTL: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Discover,
    Queue,
    Completed,
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
        }
    }
}

#[derive(Clone)]
enum Retry {
    Search(SearchQuery),
    Resolve(String),
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
}

impl From<String> for Banner {
    fn from(message: String) -> Self {
        Self {
            message,
            hint: None,
            code: None,
            retry: None,
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
}

impl From<String> for Notice {
    fn from(text: String) -> Self {
        Self {
            text,
            open_about: false,
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
    cover_index: HashMap<u64, String>,
    login_remember: bool,
    settings_jump: Option<usize>,
    retry_resolve: Option<String>,
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
            cover_index: HashMap::new(),
            login_remember: false,
            settings_jump: None,
            retry_resolve: None,
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
        self.prefs = GuiPrefs {
            output,
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
        let base = theme::background();
        if !covers_image || !self.background.has_image() {
            return base;
        }
        let alpha = (238.0 - self.prefs.opacity * 0.85).clamp(120.0, 255.0);
        theme::alpha(base, alpha / 255.0)
    }

    fn available(&self) -> bool {
        self.ready && self.busy.is_none() && !self.closing && !self.stopped
    }

    fn send(&mut self, operation: Operation) {
        if !self.available() {
            return;
        }
        let kind = match &operation {
            Operation::Search(_) => {
                self.search.start_wait();
                Busy::Search
            }
            Operation::Resolve(_) => Busy::Resolve,
            Operation::Enqueue { .. } => Busy::Enqueue,
            Operation::Login { remember } => {
                self.login_remember = *remember;
                Busy::Login
            }
            Operation::RestoreSession => Busy::Restore,
            Operation::Logout => Busy::Logout,
            Operation::ValidateOutput(_) => Busy::Output,
        };
        if let Operation::Resolve(input) = &operation {
            self.retry_resolve = Some(input.clone());
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
            Retry::Resolve(input) => self.send(Operation::Resolve(input)),
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
        while let Some(event) = self.next_event() {
            match event {
                Event::Ready => {
                    self.ready = true;
                    self.initializing = false;
                    self.init_error = None;
                    if self.prefs.remember_login {
                        self.send(Operation::RestoreSession);
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
                    let was_login = self.busy == Some(Busy::Login);
                    let was_restore = self.busy == Some(Busy::Restore);
                    let was_logout = self.busy == Some(Busy::Logout);
                    let was_search = self.busy == Some(Busy::Search);
                    let was_resolve = self.busy == Some(Busy::Resolve);
                    let was_output = self.busy == Some(Busy::Output);
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
                            for page in &results.pages {
                                for item in &page.items {
                                    if let (ContentRef::Video(aid), Some(cover)) =
                                        (&item.reference, &item.cover)
                                    {
                                        self.cover_index.insert(aid.get(), cover.clone());
                                    }
                                }
                            }
                            self.search.accept_results(results);
                            self.detail = None;
                            self.page = Page::Discover;
                        }
                        Ok(Outcome::Detail(item)) => {
                            if let Some(cover) = &item.cover {
                                for part in &item.parts {
                                    self.cover_index
                                        .insert(part.reference.aid.get(), cover.clone());
                                }
                            }
                            self.detail = Some(DetailState::new(item));
                            self.page = Page::Discover;
                        }
                        Ok(Outcome::Enqueued {
                            count,
                            cancelled,
                            error,
                        }) => {
                            self.inform(format!(
                                "已添加 {count} 项到队列。{}",
                                if cancelled {
                                    "已停止添加后续分集；已添加的任务仍在队列中。"
                                } else {
                                    ""
                                }
                            ));
                            if let Some(error) = error {
                                self.raise(control_banner(&error));
                            }
                            if count > 0 {
                                self.page = Page::Queue;
                            }
                        }
                        Ok(Outcome::Auth(snapshot)) => {
                            if was_login && snapshot.logged_in {
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
                                self.inform("已退出登录，并清除保存的登录凭据。");
                            }
                            self.auth = Some(snapshot);
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
                                banner.code = Some(backend::error_code(&error));
                                banner.hint = if retry_query.as_ref().is_some_and(|query| {
                                    query.category == bili_core::SearchCategory::All
                                }) {
                                    Some("「全部」会同时查询视频、番剧与影视，任一分区失败时整次搜索会失败；可切换到「视频」后重试。".into())
                                } else {
                                    None
                                };
                                banner.retry = if let Some(query) = retry_query {
                                    Some(Retry::Search(query))
                                } else if was_resolve {
                                    self.retry_resolve.clone().map(Retry::Resolve)
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

    fn sidebar(&mut self, ctx: &egui::Context) {
        let fill = self.surface(self.background.visible_for(&self.prefs, true));
        egui::SidePanel::left("navigation")
            .exact_width(SIDEBAR_WIDTH)
            .resizable(false)
            .frame(egui::Frame::none().fill(fill).inner_margin(18.0))
            .show(ctx, |ui| {
                ui.add_space(24.0);
                ui.label(
                    RichText::new("你的工作空间")
                        .size(11.0)
                        .color(theme::muted_color()),
                );
                ui.add_space(6.0);
                for (page, name, icon) in [
                    (Page::Discover, "搜索发现", Icon::Search),
                    (Page::Queue, "下载队列", Icon::Download),
                    (Page::Completed, "已完成", Icon::Check),
                    (Page::Settings, "设置", Icon::Settings),
                    (Page::About, "应用", Icon::Info),
                ] {
                    let width = ui.available_width();
                    if theme::button_width(
                        ui,
                        name,
                        icon,
                        if self.page == page {
                            ButtonKind::Soft
                        } else {
                            ButtonKind::Quiet
                        },
                        width,
                        self.reduced,
                    )
                    .clicked()
                    {
                        if self.page != page {
                            self.error = None;
                            if self
                                .notice
                                .as_ref()
                                .is_some_and(|notice| !notice.open_about)
                            {
                                self.notice = None;
                            }
                        }
                        self.page = page;
                    }
                    ui.add_space(2.0);
                }
                let card_height = 78.0;
                ui.add_space((ui.available_height() - card_height - 40.0).max(0.0));
                self.sidebar_account(ui);
                ui.add_space(10.0);
                ui.label(
                    RichText::new("NATIVE DESKTOP  /  0.1")
                        .size(10.0)
                        .color(theme::muted_color()),
                );
            });
    }

    fn sidebar_account(&mut self, ui: &mut Ui) {
        let logged_in = self.auth.as_ref().is_some_and(|a| a.logged_in);
        let name = self
            .auth
            .as_ref()
            .and_then(|a| a.account.as_ref())
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "访客模式".into());
        let face = self
            .auth
            .as_ref()
            .and_then(|a| a.account.as_ref())
            .and_then(|a| a.face.as_deref())
            .map(normalize_face);
        let card = theme::inset(theme::card()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let size = egui::vec2(40.0, 40.0);
                if !self
                    .covers
                    .show(ui, self.bridge.as_ref(), face.as_deref(), size, 20.0)
                {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter()
                        .circle_filled(rect.center(), 20.0, theme::soft());
                    theme::icon(
                        ui.painter(),
                        egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0)),
                        Icon::User,
                        theme::primary(),
                    );
                }
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(RichText::new(&name).strong()).truncate())
                        .on_hover_text(&name);
                    theme::muted(
                        ui,
                        if logged_in {
                            if self.prefs.remember_login {
                                "保持登录"
                            } else {
                                "本次会话已登录"
                            }
                        } else {
                            "尚未登录，使用访客画质"
                        },
                    );
                });
            });
        });
        let response = card
            .response
            .interact(egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if logged_in {
                "查看当前账号信息"
            } else {
                "点击登录"
            });
        if response.hovered() {
            ui.painter().rect_stroke(
                response.rect,
                16.0,
                egui::Stroke::new(1.0_f32, theme::alpha(theme::primary(), 0.35)),
            );
        }
        if response.clicked() {
            self.login.open = true;
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
                    });
                    ui.add_space(8.0);
                }
                if let Some(notice) = self.notice.clone().filter(|notice| {
                    !(notice.open_about && (self.error.is_some() || self.page == Page::About))
                }) {
                    theme::inset(theme::soft()).show(ui, |ui| {
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
                                self.page = Page::About;
                                self.notice = None;
                                self.notice_at = None;
                            }
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
                        (BackgroundScope::WithoutSidebar, "不含左侧边"),
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
                    egui::TextEdit::singleline(&mut self.output_input)
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
        let central_fill = self.surface(true);
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(central_fill).inner_margin(28.0))
            .show(ctx, |ui| {
                ui.add_enabled_ui(!self.closing, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            let (title, subtitle) = match self.page {
                                Page::Discover if self.detail.is_some() => {
                                    ("准备下载", "选择分集、字幕与保存位置。")
                                }
                                Page::Discover => {
                                    ("发现喜欢的每一帧", "搜索视频与番剧，开启你的本地收藏。")
                                }
                                Page::Queue => ("下载队列", "每一份喜欢，都在有序抵达。"),
                                Page::Completed => ("已完成", "下载已就绪，文件保存在你的设备上。"),
                                Page::Settings if self.background.crop_open => {
                                    ("裁剪背景", "选中你想留下的画面，应用后立即生效。")
                                }
                                Page::Settings => ("偏好设置", "简单一点，刚刚好。"),
                                Page::About => ("应用", "版本、更新与项目信息。"),
                            };
                            ui.label(RichText::new(title).size(28.0).strong());
                            theme::muted(ui, subtitle);
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.page == Page::Settings
                                && !self.background.crop_open
                                && theme::button(
                                    ui,
                                    "恢复默认外观",
                                    Icon::Refresh,
                                    ButtonKind::Quiet,
                                    self.reduced,
                                )
                                .clicked()
                            {
                                self.reset_appearance();
                            }
                            let active = self
                                .snapshot
                                .jobs
                                .iter()
                                .filter(|j| j.phase.is_active())
                                .count();
                            if active > 0 {
                                if theme::button(
                                    ui,
                                    &format!("{active} 项进行中"),
                                    Icon::Download,
                                    ButtonKind::Soft,
                                    self.reduced,
                                )
                                .clicked()
                                {
                                    self.page = Page::Queue;
                                }
                            } else {
                                ui.label(
                                    RichText::new(if self.ready {
                                        "本地就绪"
                                    } else {
                                        "原生桌面"
                                    })
                                    .size(12.0)
                                    .color(theme::muted_color()),
                                );
                            }
                        });
                    });
                    ui.add_space(14.0);
                    self.banners(ui);
                    if self.page == Page::Settings && !self.background.crop_open {
                        self.settings_nav(ui);
                    }
                    ui.add_space(10.0);
                    let mut scroll = egui::ScrollArea::vertical()
                        .id_salt(match self.page {
                            Page::Discover if self.detail.is_some() => "main-scroll-detail",
                            Page::Discover => "main-scroll-discover",
                            Page::Queue => "main-scroll-queue",
                            Page::Completed => "main-scroll-completed",
                            Page::Settings => "main-scroll-settings",
                            Page::About => "main-scroll-about",
                        })
                        .auto_shrink([false, false]);
                    if self.background.crop_open {
                        scroll = scroll.vertical_scroll_offset(0.0);
                    }
                    scroll.show(ui, |ui| {
                        content_column(ui, |ui| {
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
                                            self.detail = None;
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
                                    if let Some(action) = action {
                                        self.send(action);
                                    }
                                }
                                Page::Queue | Page::Completed => {
                                    let control_available =
                                        self.ready && !self.closing && !self.stopped;
                                    if let Some((id, action)) = self.queue.show(
                                        ui,
                                        crate::download_panel::QueueView {
                                            snapshot: &self.snapshot,
                                            completed: self.page == Page::Completed,
                                            pending: &self.controls,
                                            available: control_available,
                                            reduced: self.reduced,
                                            cover_index: &self.cover_index,
                                        },
                                        &mut self.covers,
                                        self.bridge.as_ref(),
                                    ) {
                                        if let Some(bridge) = &self.bridge {
                                            match bridge.send(Command::Control(id, action)) {
                                                Ok(()) => {
                                                    self.controls.insert(id);
                                                }
                                                Err(error) => self.raise(error),
                                            }
                                        }
                                    }
                                }
                                Page::Settings => {
                                    if self.background.crop_open {
                                        if self.background.show_crop(ui, &self.prefs)
                                            == CropEvent::Applied
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
                            ui.add_space(18.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new("仅下载你有权保存的内容")
                                        .size(11.0)
                                        .color(theme::muted_color()),
                                );
                                ui.label(
                                    RichText::new("·  Ctrl+K 快速输入")
                                        .size(11.0)
                                        .color(theme::muted_color()),
                                );
                                let completed = self
                                    .snapshot
                                    .jobs
                                    .iter()
                                    .filter(|j| j.phase == JobPhase::Completed)
                                    .count();
                                if completed > 0 {
                                    ui.label(
                                        RichText::new(format!("·  {completed} 项已完成"))
                                            .size(11.0)
                                            .color(theme::muted_color()),
                                    );
                                }
                            });
                        });
                    });
                });
            });
        self.toasts(ctx);
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
    banner
}

/// Keeps reading width comfortable on wide screens while staying full-width
/// on small ones; scroll position and hit testing remain inside the column.
fn content_column<R>(ui: &mut Ui, body: impl FnOnce(&mut Ui) -> R) -> R {
    let available = ui.available_width();
    let width = available.min(CONTENT_MAX_WIDTH);
    if available - width < 1.0 {
        return body(ui);
    }
    let pad = (available - width) * 0.5;
    ui.horizontal_top(|ui| {
        ui.add_space(pad);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            body,
        )
        .inner
    })
    .inner
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) && !self.closing
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
            self.page = Page::Discover;
            self.detail = None;
            self.search.request_focus = true;
        }
        let screen = ctx.screen_rect();
        let clip = if self.prefs.background_scope == BackgroundScope::Full {
            screen
        } else {
            egui::Rect::from_min_max(
                egui::pos2(screen.left() + SIDEBAR_WIDTH, screen.top()),
                screen.max,
            )
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
        self.sidebar(ctx);
        self.central(ctx);
        self.toasts(ctx);
        let available = self.available();
        let login_running = self.busy == Some(Busy::Login);
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
                LoginAction::Start { remember } => self.send(Operation::Login { remember }),
                LoginAction::Cancel => self.cancel(),
                LoginAction::Logout => self.send(Operation::Logout),
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
        Aid, Bvid, Candidate, CandidatePage, CategoryPage, Cid, ContentRef, JobId, JobSnapshot,
        MediaItem, Part, PartRef, SearchCategory,
    };
    use egui::{Pos2, Rect, Vec2};

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
            app.sidebar(ctx);
            app.central(ctx);
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

    fn results(count: usize) -> CandidatePage {
        CandidatePage {
            pages: vec![CategoryPage {
                category: SearchCategory::Video,
                page: 1,
                total: Some(count as u64),
                has_more: false,
                items: (0..count)
                    .map(|index| Candidate {
                        reference: ContentRef::Video(Aid::new(1_000 + index as u64).unwrap()),
                        category: SearchCategory::Video,
                        title: format!("测试视频 {index}"),
                        cover: None,
                        duration_ms: Some(61_000),
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
        app.search.results = Some(results(12));

        let output = frame(&mut ctx, &mut app, size, Vec::new());
        let before = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text.contains("搜索结果"))
            .map(|(_, rect, _)| rect)
            .expect("search header");

        let mut events = Vec::new();
        wheel(&mut events, egui::pos2(600.0, 500.0), 3);
        frame(&mut ctx, &mut app, size, events);
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        let after = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text.contains("搜索结果"))
            .map(|(_, rect, _)| rect)
            .expect("search header after scrolling");
        assert!(
            after.top() < before.top() - 50.0,
            "search results did not scroll: before {before:?} after {after:?}"
        );

        app.detail = Some(DetailState::new(media_item()));
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert_visible(&output, "返回搜索", screen);

        app.detail = None;
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        let restored = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text.contains("搜索结果"))
            .map(|(_, rect, _)| rect)
            .expect("search header after going back");
        assert!(
            (restored.top() - after.top()).abs() < 40.0,
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
        app.page = Page::Completed;
        app.snapshot = Arc::new(QueueSnapshot {
            schema_version: 1,
            revision: 1,
            jobs: vec![Arc::new(job(
                JobPhase::Completed,
                Some(r"C:\Users\Public\bili-test.mp4"),
            ))],
        });
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        assert_visible(&output, "复制文件路径", screen);

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

    #[test]
    fn sidebar_account_card_keeps_name_status_and_action_order() {
        let size = Vec2::new(1180.0, 800.0);
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        let find = |needle: &str| {
            texts(&output)
                .into_iter()
                .find(|(text, _, _)| text.contains(needle))
                .map(|(_, rect, _)| rect)
                .unwrap_or_else(|| panic!("missing text: {needle}"))
        };
        let name = find("访客模式");
        let status = find("尚未登录，使用访客画质");
        assert!(name.top() < status.top(), "name must sit above status");
        assert!(
            !texts(&output)
                .iter()
                .any(|(text, _, _)| text.contains("扫码登录")),
            "the account card must not repeat a login button"
        );
        let native = find("NATIVE DESKTOP");
        assert!(
            native.top() - status.bottom() < 80.0,
            "account card is stretched: status {status:?} native {native:?}"
        );
        for rect in [name, status] {
            assert!(
                screen.contains_rect(rect),
                "account card item is clipped: {rect:?}"
            );
        }
    }

    #[test]
    fn signed_in_account_card_fits_small_windows() {
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        app.auth = Some(bili_core::AuthSnapshot {
            logged_in: true,
            account: Some(bili_core::auth::AuthAccount {
                mid: 1,
                name: "测试账号".into(),
                face: None,
            }),
            notice: None,
            qr: None,
            qr_status: None,
        });
        app.prefs.remember_login = true;
        for size in [
            Vec2::new(900.0, 640.0),
            Vec2::new(1180.0, 800.0),
            Vec2::new(1706.0, 960.0),
        ] {
            let screen = Rect::from_min_size(Pos2::ZERO, size);
            frame(&mut ctx, &mut app, size, Vec::new());
            let output = frame(&mut ctx, &mut app, size, Vec::new());
            let all = texts(&output);
            let rect = |needle: &str| {
                all.iter()
                    .find(|(text, _, _)| text.contains(needle))
                    .map(|(_, rect, _)| *rect)
                    .unwrap_or_else(|| panic!("missing {needle} at {size:?}"))
            };
            let status = rect("保持登录");
            let native = rect("NATIVE DESKTOP");
            assert!(
                native.top() - status.bottom() < 80.0,
                "card is stretched at {size:?}: {status:?} {native:?}"
            );
            for rect in [status, native] {
                assert!(screen.contains_rect(rect), "clipped at {size:?}: {rect:?}");
            }
        }
    }

    #[test]
    fn clicking_the_account_card_opens_the_login_dialog() {
        let size = Vec2::new(1180.0, 800.0);
        let mut ctx = egui::Context::default();
        let mut app = test_app();
        let output = frame(&mut ctx, &mut app, size, Vec::new());
        let name = texts(&output)
            .into_iter()
            .find(|(text, _, _)| text.contains("访客模式"))
            .map(|(_, rect, _)| rect)
            .expect("account name");
        let pos = name.center();
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
                    modifiers: egui::Modifiers::default(),
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
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert!(
            app.login.open,
            "clicking the account card must open the login dialog"
        );
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
