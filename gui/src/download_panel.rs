//! Project-oriented projections of immutable core history; mutations stay in core.
use crate::{
    backend::{self, Bridge, Operation},
    covers::CoverCache,
    search, theme,
};
use bili_core::{
    ErrorCode, JobAction, JobId, JobPhase, JobSnapshot, OutputInfo, QueueSnapshot, RemoveMode,
    SearchCategory,
};
use egui::{RichText, Ui};
use std::{collections::HashSet, path::PathBuf, sync::Arc};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    Active,
    Failed,
    Completed,
}
fn status(job: &JobSnapshot) -> Filter {
    if matches!(job.phase, JobPhase::Failed | JobPhase::NeedsAttention)
        || job.components.iter().any(|c| c.error.is_some())
    {
        Filter::Failed
    } else if job.phase == JobPhase::Completed {
        Filter::Completed
    } else {
        Filter::Active
    }
}
pub fn phase_name(phase: JobPhase) -> &'static str {
    match phase {
        JobPhase::Queued => "排队中",
        JobPhase::Resolving => "正在解析",
        JobPhase::Downloading => "正在下载",
        JobPhase::Downloaded => "等待封装",
        JobPhase::Remuxing => "正在封装",
        JobPhase::Verifying => "正在校验",
        JobPhase::Committing => "正在保存",
        JobPhase::Completed => "已完成",
        JobPhase::Pausing => "正在保存断点",
        JobPhase::Paused => "已暂停，可继续",
        JobPhase::Cancelling => "正在取消",
        JobPhase::Cancelled => "已停止，可继续",
        JobPhase::Failed | JobPhase::NeedsAttention => "失败",
    }
}
pub fn bytes(value: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut n = value as f64;
    let mut u = 0;
    while n >= 1024.0 && u < 4 {
        n /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{value} B")
    } else {
        format!("{n:.1} {}", U[u])
    }
}
pub fn fraction(downloaded: u64, total: Option<u64>) -> Option<f32> {
    total
        .filter(|n| *n > 0)
        .map(|n| (downloaded as f64 / n as f64).clamp(0.0, 1.0) as f32)
}
pub struct QueueView<'a> {
    pub snapshot: &'a QueueSnapshot,
    pub pending: &'a HashSet<JobId>,
    pub available: bool,
    pub operations_available: bool,
    pub reduced: bool,
    pub current_account: Option<&'a bili_core::auth::AuthProfile>,
}
pub enum QueueAction {
    Control(JobId, JobAction),
    Operation(Operation),
    Account(JobId),
}
#[derive(Default)]
pub struct QueueState {
    filter: Filter,
    kind: Option<SearchCategory>,
    oldest_first: bool,
    project: Option<String>,
    info: Option<OutputInfo>,
    delete: Option<DeleteIntent>,
    delete_files: bool,
}
struct DeleteIntent {
    ids: Vec<JobId>,
    title: String,
    project: Option<String>,
}
struct Project {
    key: String,
    title: String,
    cover: Option<String>,
    jobs: Vec<Arc<JobSnapshot>>,
}
fn job_title(job: &JobSnapshot) -> String {
    job.plan
        .as_ref()
        .map(|p| p.title.clone())
        .or_else(|| {
            job.output
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| format!("任务 {}", job.id))
}
fn project_key(job: &JobSnapshot) -> String {
    job.plan
        .as_ref()
        .and_then(|p| p.context.as_ref())
        .and_then(|c| c.project.as_ref())
        .map_or_else(
            || format!("legacy-{}", job.id),
            |p| search::reference_input(&p.reference),
        )
}
fn project_kind(job: &JobSnapshot) -> Option<SearchCategory> {
    job.plan
        .as_ref()
        .and_then(|p| p.context.as_ref())
        .and_then(|c| c.project.as_ref())
        .and_then(|p| p.category)
}
impl QueueState {
    pub fn select_filter(&mut self, filter: Filter) {
        self.filter = filter;
        self.project = None;
    }
    pub fn accept_info(&mut self, info: OutputInfo) {
        self.info = Some(info);
    }
    pub fn show(
        &mut self,
        ui: &mut Ui,
        view: QueueView<'_>,
        covers: &mut CoverCache,
        bridge: Option<&Bridge>,
    ) -> Option<QueueAction> {
        let QueueView {
            snapshot,
            pending,
            available,
            operations_available,
            reduced,
            current_account,
        } = view;
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            for (filter, label) in [
                (Filter::Active, "进行中"),
                (Filter::Failed, "失败"),
                (Filter::Completed, "已完成"),
            ] {
                let count = snapshot.jobs.iter().filter(|j| status(j) == filter).count();
                if ui
                    .selectable_label(self.filter == filter, format!("{label} {count}"))
                    .clicked()
                {
                    self.select_filter(filter);
                }
            }
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("history-kind")
                .selected_text(self.kind.map_or("全部类型", search::category_name))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.kind, None, "全部类型");
                    for kind in [
                        SearchCategory::Video,
                        SearchCategory::Bangumi,
                        SearchCategory::Film,
                    ] {
                        ui.selectable_value(
                            &mut self.kind,
                            Some(kind),
                            search::category_name(kind),
                        );
                    }
                });
            if self.project.is_none() {
                egui::ComboBox::from_id_salt("history-order")
                    .selected_text(if self.oldest_first {
                        "最早添加在前"
                    } else {
                        "最新添加在前"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.oldest_first, false, "最新添加在前");
                        ui.selectable_value(&mut self.oldest_first, true, "最早添加在前");
                    });
            }
        });
        let mut projects: Vec<Project> = Vec::new();
        for job in &snapshot.jobs {
            if status(job) != self.filter
                || self
                    .kind
                    .is_some_and(|kind| project_kind(job) != Some(kind))
            {
                continue;
            }
            let key = project_key(job);
            if let Some(project) = projects.iter_mut().find(|p| p.key == key) {
                project.jobs.push(job.clone());
            } else {
                let meta = job
                    .plan
                    .as_ref()
                    .and_then(|p| p.context.as_ref())
                    .and_then(|c| c.project.as_ref());
                projects.push(Project {
                    key,
                    title: meta.map_or_else(|| job_title(job), |p| p.title.clone()),
                    cover: meta.and_then(|p| p.cover.clone()),
                    jobs: vec![job.clone()],
                });
            }
        }
        projects.sort_by(|a, b| {
            let x = a.jobs.iter().filter_map(|j| j.created_at).max();
            let y = b.jobs.iter().filter_map(|j| j.created_at).max();
            match (x, y) {
                (Some(x), Some(y)) => {
                    if self.oldest_first {
                        x.cmp(&y)
                    } else {
                        y.cmp(&x)
                    }
                }
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, None) => a.key.cmp(&b.key),
            }
        });
        ui.add_space(12.0);
        if let Some(key) = self.project.clone() {
            if ui.button("返回项目").clicked() {
                self.project = None;
            }
            if let Some(project) = projects.iter_mut().find(|p| p.key == key) {
                ui.label(RichText::new(&project.title).strong());
                project.jobs.sort_by_key(|j| {
                    j.plan
                        .as_ref()
                        .and_then(|p| p.context.as_ref())
                        .and_then(|c| c.project.as_ref())
                        .map(|p| p.ordinal)
                });
                for job in &project.jobs {
                    ui.push_id(job.id, |ui| {
                        if let Some(next) = self.job(
                            ui,
                            job,
                            available && !pending.contains(&job.id),
                            operations_available,
                            reduced,
                            current_account,
                        ) {
                            action = Some(next);
                        }
                    });
                }
            } else {
                theme::muted(ui, "该项目在当前筛选下没有任务。");
            }
        } else {
            if projects.is_empty() {
                theme::muted(ui, "没有符合条件的任务。");
            }
            for project in &projects {
                if project.jobs.len() == 1
                    && snapshot
                        .jobs
                        .iter()
                        .filter(|j| project_key(j) == project.key)
                        .count()
                        == 1
                {
                    let job = &project.jobs[0];
                    ui.push_id(job.id, |ui| {
                        if let Some(next) = self.job(
                            ui,
                            job,
                            available && !pending.contains(&job.id),
                            operations_available,
                            reduced,
                            current_account,
                        ) {
                            action = Some(next);
                        }
                    });
                    continue;
                }
                egui::Frame::group(ui.style())
                    .fill(theme::card())
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            let size = egui::vec2(112.0, 63.0);
                            if !covers.show(ui, bridge, project.cover.as_deref(), size, 4.0) {
                                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                                ui.painter().rect_filled(rect, 4.0, theme::soft());
                            }
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&project.title).strong());
                                theme::muted(ui, format!("{} 项", project.jobs.len()));
                                if project.jobs.iter().all(|j| j.created_at.is_none()) {
                                    theme::muted(ui, "旧记录：添加时间未知");
                                }
                                if project.jobs.iter().any(|j| {
                                    matches!(j.phase, JobPhase::Paused | JobPhase::Cancelled)
                                }) {
                                    theme::muted(ui, "含已暂停任务");
                                }
                            });
                        });
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("进入项目").clicked() {
                                self.project = Some(project.key.clone());
                            }
                            let directories: Vec<PathBuf> = project
                                .jobs
                                .iter()
                                .filter_map(|j| {
                                    j.output
                                        .as_ref()
                                        .or_else(|| j.plan.as_ref().map(|p| &p.destination))
                                })
                                .filter_map(|p| p.parent().map(ToOwned::to_owned))
                                .collect::<std::collections::BTreeSet<_>>()
                                .into_iter()
                                .collect();
                            if directories.len() == 1 {
                                if ui
                                    .add_enabled(
                                        operations_available,
                                        egui::Button::new("打开文件夹"),
                                    )
                                    .clicked()
                                {
                                    action = Some(QueueAction::Operation(Operation::Reveal {
                                        path: directories[0].clone(),
                                        select: false,
                                    }));
                                }
                            } else if !directories.is_empty() {
                                ui.menu_button("打开文件夹", |ui| {
                                    for path in &directories {
                                        if ui
                                            .add_enabled(
                                                operations_available,
                                                egui::Button::new(path.display().to_string()),
                                            )
                                            .clicked()
                                        {
                                            action =
                                                Some(QueueAction::Operation(Operation::Reveal {
                                                    path: path.clone(),
                                                    select: false,
                                                }));
                                            ui.close_menu();
                                        }
                                    }
                                });
                            }
                            if ui
                                .add_enabled(available, egui::Button::new("删除项目"))
                                .clicked()
                            {
                                self.delete = Some(DeleteIntent {
                                    ids: snapshot
                                        .jobs
                                        .iter()
                                        .filter(|j| project_key(j) == project.key)
                                        .map(|j| j.id)
                                        .collect(),
                                    title: project.title.clone(),
                                    project: Some(project.key.clone()),
                                });
                                self.delete_files = false;
                            }
                        });
                    });
                ui.add_space(8.0);
            }
        }
        if let Some(next) = self.dialogs(ui.ctx(), snapshot, operations_available) {
            action = Some(next);
        }
        action
    }
    fn job(
        &mut self,
        ui: &mut Ui,
        job: &JobSnapshot,
        available: bool,
        operations_available: bool,
        reduced: bool,
        current_account: Option<&bili_core::auth::AuthProfile>,
    ) -> Option<QueueAction> {
        let mut action = None;
        egui::Frame::group(ui.style())
            .fill(theme::card())
            .show(ui, |ui| {
                ui.label(RichText::new(job_title(job)).strong());
                theme::muted(ui, phase_name(job.phase));
                if let Some(plan) = &job.plan {
                    theme::muted(
                        ui,
                        format!(
                            "视频尺寸：{} × {} · {} · MP4",
                            plan.video
                                .width
                                .map_or_else(|| "?".into(), |n| n.to_string()),
                            plan.video
                                .height
                                .map_or_else(|| "?".into(), |n| n.to_string()),
                            plan.video.codec
                        ),
                    );
                }
                if job.phase != JobPhase::Completed {
                    theme::progress(
                        ui,
                        ui.id().with((job.id, "progress")),
                        fraction(job.downloaded, job.total),
                        job.phase.is_active(),
                        reduced,
                    );
                    theme::muted(ui, format!("已下载 {}", bytes(job.downloaded)));
                }
                let error = job
                    .error
                    .as_ref()
                    .or_else(|| job.components.iter().find_map(|c| c.error.as_ref()));
                if let Some(error) = error {
                    ui.colored_label(theme::error_color(), backend::error_message(error));
                    ui.collapsing("错误信息", |ui| {
                        ui.label(backend::error_code(error));
                        ui.label(&error.operation);
                        if let Some(code) = error.native_code {
                            ui.label(format!("原生错误码 {code}"));
                        }
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(available, |ui| {
                        if job.phase.is_active() {
                            if ui.button("暂停").clicked() {
                                action = Some(QueueAction::Control(job.id, JobAction::Pause));
                            }
                        } else if matches!(job.phase, JobPhase::Paused | JobPhase::Cancelled) {
                            if ui.button("继续下载").clicked() {
                                action = Some(QueueAction::Control(job.id, JobAction::Resume));
                            }
                        } else if status(job) == Filter::Failed {
                            match error.map(|e| e.code) {
                                Some(ErrorCode::LoginRequired | ErrorCode::EntitlementRequired) => {
                                    if let Some(profile) = current_account {
                                        if ui
                                            .button(format!(
                                                "使用当前账号重试（{}）",
                                                profile.account.name
                                            ))
                                            .clicked()
                                        {
                                            action = Some(QueueAction::Operation(
                                                Operation::RetargetProfile {
                                                    id: job.id,
                                                    profile: Some(profile.id.clone()),
                                                },
                                            ));
                                        }
                                    }
                                    if ui.button("选择账号").clicked() {
                                        action = Some(QueueAction::Account(job.id));
                                    }
                                }
                                Some(ErrorCode::VerificationRequired) => {
                                    if let Some(request) =
                                        error.and_then(|e| e.verification.clone())
                                    {
                                        if ui
                                            .add_enabled(
                                                operations_available,
                                                egui::Button::new("安全验证"),
                                            )
                                            .clicked()
                                        {
                                            action = Some(QueueAction::Operation(
                                                Operation::BeginVerification {
                                                    request,
                                                    profile: job
                                                        .plan
                                                        .as_ref()
                                                        .and_then(|p| p.context.as_ref())
                                                        .and_then(|c| c.profile_id.clone()),
                                                },
                                            ));
                                        }
                                    }
                                    if ui.button("重试").clicked() {
                                        action =
                                            Some(QueueAction::Control(job.id, JobAction::Retry));
                                    }
                                }
                                Some(ErrorCode::RepresentationChanged)
                                    if error.is_some_and(|e| {
                                        e.operation == "selected representation verification"
                                    }) =>
                                {
                                    if ui.button("重新校验与封装").clicked() {
                                        action =
                                            Some(QueueAction::Control(job.id, JobAction::Retry));
                                    }
                                }
                                Some(
                                    ErrorCode::Unavailable
                                    | ErrorCode::RegionRestricted
                                    | ErrorCode::RepresentationChanged,
                                ) => {}
                                _ => {
                                    if ui
                                        .button(if job.phase == JobPhase::Completed {
                                            "重试附件"
                                        } else {
                                            "重试"
                                        })
                                        .clicked()
                                    {
                                        action =
                                            Some(QueueAction::Control(job.id, JobAction::Retry));
                                    }
                                }
                            }
                        }
                        if let Some(path) = &job.output {
                            if ui
                                .add_enabled(
                                    operations_available,
                                    egui::Button::new("打开文件所在位置"),
                                )
                                .clicked()
                            {
                                action = Some(QueueAction::Operation(Operation::Reveal {
                                    path: path.clone(),
                                    select: true,
                                }));
                            }
                            if ui
                                .add_enabled(operations_available, egui::Button::new("媒体信息"))
                                .clicked()
                            {
                                action =
                                    Some(QueueAction::Operation(Operation::OutputInfo(job.id)));
                            }
                        }
                        if ui.button("删除").clicked() {
                            self.delete = Some(DeleteIntent {
                                ids: vec![job.id],
                                title: job_title(job),
                                project: None,
                            });
                            self.delete_files = false;
                        }
                    });
                });
                for component in &job.components {
                    if component.error.is_some() {
                        ui.colored_label(
                            theme::error_color(),
                            format!("{} 下载失败", component.name),
                        );
                    } else if let Some(path) = &component.path {
                        theme::muted(
                            ui,
                            format!(
                                "{} · {}",
                                component.name,
                                path.extension()
                                    .and_then(|p| p.to_str())
                                    .unwrap_or("格式未知")
                            ),
                        );
                    }
                }
            });
        ui.add_space(7.0);
        action
    }
    fn dialogs(
        &mut self,
        ctx: &egui::Context,
        snapshot: &QueueSnapshot,
        available: bool,
    ) -> Option<QueueAction> {
        let mut action = None;
        if let Some(info) = &self.info {
            let mut open = true;
            egui::Window::new("媒体信息")
                .open(&mut open)
                .collapsible(false)
                .default_width(560.0)
                .show(ctx, |ui| {
                    if let Some(path) = &info.output {
                        ui.label(path.display().to_string());
                    }
                    if let Some(media) = &info.media {
                        ui.label(format!(
                            "容器：{}",
                            media.format.as_deref().unwrap_or("未知")
                        ));
                        for stream in &media.streams {
                            ui.label(format!("{} · {}", stream.media_type, stream.codec));
                            if let (Some(w), Some(h)) = (stream.width, stream.height) {
                                ui.label(format!("实际尺寸：{w} × {h}"));
                            }
                            if let (Some(n), Some(d)) =
                                (stream.frame_rate_num, stream.frame_rate_den)
                            {
                                if d > 0 {
                                    ui.label(format!("帧率：{:.3}", n as f64 / d as f64));
                                }
                            }
                        }
                    } else {
                        theme::muted(ui, "媒体参数不可用。");
                    }
                    for component in &info.components {
                        if let Some(path) = &component.path {
                            ui.label(format!(
                                "{}：{}",
                                component.name,
                                path.extension()
                                    .and_then(|p| p.to_str())
                                    .unwrap_or("未知格式")
                            ));
                        }
                    }
                });
            if !open {
                self.info = None;
            }
        }
        if let Some(intent) = &self.delete {
            let mut open = true;
            let mut confirmed = false;
            let mut cancelled = false;
            let ids = intent.project.as_ref().map_or_else(
                || intent.ids.clone(),
                |project| {
                    snapshot
                        .jobs
                        .iter()
                        .filter(|j| project_key(j) == *project)
                        .map(|j| j.id)
                        .collect::<Vec<_>>()
                },
            );
            let active = ids.iter().any(|id| {
                snapshot
                    .jobs
                    .iter()
                    .any(|j| j.id == *id && j.phase.is_active())
            });
            egui::Window::new("删除下载记录")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(&intent.title);
                    ui.label(format!("共 {} 条记录，包含此项目的所有状态。", ids.len()));
                    ui.radio_value(&mut self.delete_files, false, "仅移除记录，保留文件");
                    ui.radio_value(
                        &mut self.delete_files,
                        true,
                        "同时将任务拥有的文件移至回收站",
                    );
                    if active {
                        ui.colored_label(theme::error_color(), "请先暂停此范围内的下载，再删除。");
                    }
                    ui.horizontal(|ui| {
                        confirmed = ui
                            .add_enabled(available && !active, egui::Button::new("确认删除"))
                            .clicked();
                        cancelled = ui.button("取消").clicked();
                    });
                });
            if confirmed {
                action = Some(QueueAction::Operation(Operation::Remove {
                    ids,
                    mode: if self.delete_files {
                        RemoveMode::RecycleFiles
                    } else {
                        RemoveMode::RecordsOnly
                    },
                }));
            }
            if confirmed || cancelled || !open {
                self.delete = None;
            }
        }
        action
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_preserves_unknown_totals_and_clamps() {
        assert_eq!(fraction(5, None), None);
        assert_eq!(fraction(50, Some(10)), Some(1.0));
        assert_eq!(fraction(5, Some(10)), Some(0.5));
    }
}
