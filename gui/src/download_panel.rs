//! Snapshot-only queue presentation; all controls go back through core.
use crate::{
    backend::{self, Bridge},
    covers::CoverCache,
    search,
    theme::{self, ButtonKind, Icon},
};
use bili_core::{JobAction, JobId, JobPhase, JobSnapshot, QueueSnapshot};
use egui::{RichText, Ui};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::SystemTime,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Active,
    Paused,
    Attention,
}

pub fn matches_filter(phase: JobPhase, completed: bool, filter: Filter) -> bool {
    if completed {
        return phase == JobPhase::Completed;
    }
    if phase == JobPhase::Completed {
        return false;
    }
    match filter {
        Filter::All => true,
        Filter::Active => phase.is_active() || phase == JobPhase::Downloaded,
        Filter::Paused => matches!(phase, JobPhase::Paused | JobPhase::Cancelled),
        Filter::Attention => matches!(phase, JobPhase::Failed | JobPhase::NeedsAttention),
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
        JobPhase::Paused => "已暂停",
        JobPhase::Cancelling => "正在取消",
        JobPhase::Cancelled => "已取消",
        JobPhase::Failed => "下载失败",
        JobPhase::NeedsAttention => "需要处理",
    }
}

pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

pub fn fraction(downloaded: u64, total: Option<u64>) -> Option<f32> {
    total
        .filter(|&total| total > 0)
        .map(|total| (downloaded as f64 / total as f64).clamp(0.0, 1.0) as f32)
}

pub struct QueueView<'a> {
    pub snapshot: &'a QueueSnapshot,
    pub completed: bool,
    pub pending: &'a HashSet<JobId>,
    pub available: bool,
    pub reduced: bool,
    pub cover_index: &'a HashMap<u64, String>,
}

#[derive(Default)]
pub struct QueueState {
    filter: Filter,
    text: String,
    copied: Option<JobId>,
}

impl QueueState {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        view: QueueView<'_>,
        covers: &mut CoverCache,
        bridge: Option<&Bridge>,
    ) -> Option<(JobId, JobAction)> {
        let QueueView {
            snapshot,
            completed,
            pending,
            available,
            reduced,
            cover_index,
        } = view;
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            if !completed {
                for (filter, name) in [
                    (Filter::All, "全部任务"),
                    (Filter::Active, "进行中"),
                    (Filter::Paused, "已停止"),
                    (Filter::Attention, "需处理"),
                ] {
                    if theme::button(
                        ui,
                        name,
                        if self.filter == filter {
                            Icon::Check
                        } else {
                            Icon::Download
                        },
                        if self.filter == filter {
                            ButtonKind::Soft
                        } else {
                            ButtonKind::Quiet
                        },
                        reduced,
                    )
                    .clicked()
                    {
                        self.filter = filter;
                    }
                }
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .hint_text("筛选任务标题")
                    .desired_width(160.0)
                    .margin(egui::vec2(12.0, 10.0)),
            );
        });
        ui.add_space(12.0);
        let text = self.text.trim().to_lowercase();
        let jobs: Vec<_> = snapshot
            .jobs
            .iter()
            .filter(|job| {
                matches_filter(job.phase, completed, self.filter)
                    && (text.is_empty()
                        || job
                            .plan
                            .as_ref()
                            .is_some_and(|p| p.title.to_lowercase().contains(&text)))
            })
            .collect();
        if jobs.is_empty() {
            let has_jobs = snapshot
                .jobs
                .iter()
                .any(|job| matches_filter(job.phase, completed, Filter::All));
            search::empty(
                ui,
                if has_jobs {
                    "没有匹配的任务"
                } else if completed {
                    "完成之后，在这里相见"
                } else {
                    "下载队列，准备就绪"
                },
                if has_jobs {
                    "试试其他筛选条件，或清空标题搜索。"
                } else if completed {
                    "实际完成的文件会出现在这里，可一键复制保存路径。"
                } else {
                    "在「搜索发现」中选择内容，然后添加到队列。"
                },
            );
            return None;
        }
        for job in jobs {
            ui.push_id(job.id.to_string(), |ui| {
                theme::card_frame().show(ui, |ui| {
                    theme::inset(theme::card()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let size = egui::vec2(96.0, 54.0);
                            let cover = job
                                .plan
                                .as_ref()
                                .and_then(|plan| cover_index.get(&plan.part.aid.get()))
                                .map(String::as_str);
                            if !covers.show(ui, bridge, cover, size, 12.0) {
                                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                                ui.painter().rect_filled(rect, 12.0, theme::soft());
                                theme::icon(
                                    ui.painter(),
                                    egui::Rect::from_center_size(
                                        rect.center(),
                                        egui::vec2(24.0, 24.0),
                                    ),
                                    if completed {
                                        Icon::Check
                                    } else {
                                        Icon::Download
                                    },
                                    theme::primary(),
                                );
                            }
                            ui.add_space(4.0);
                            let title = job
                                .plan
                                .as_ref()
                                .map_or("本地媒体任务", |plan| plan.title.as_str());
                            ui.vertical(|ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(title).size(16.0).strong())
                                        .truncate(),
                                )
                                .on_hover_text(title);
                                let color = match job.phase {
                                    JobPhase::Completed => theme::GREEN,
                                    JobPhase::Failed | JobPhase::NeedsAttention => theme::ERROR,
                                    _ => theme::muted_color(),
                                };
                                ui.label(
                                    RichText::new(phase_name(job.phase)).color(color).size(13.0),
                                );
                            });
                        });
                        ui.add_space(8.0);
                        if job.phase != JobPhase::Completed {
                            let measured = if matches!(
                                job.phase,
                                JobPhase::Downloading
                                    | JobPhase::Paused
                                    | JobPhase::Cancelled
                                    | JobPhase::Failed
                                    | JobPhase::NeedsAttention
                            ) {
                                fraction(job.downloaded, job.total)
                            } else {
                                None
                            };
                            let progress_id = ui.id().with("progress");
                            theme::progress(
                                ui,
                                progress_id,
                                measured,
                                job.phase.is_active(),
                                reduced,
                            );
                            ui.horizontal_wrapped(|ui| {
                                theme::muted(
                                    ui,
                                    match job.total {
                                        Some(total) if total > 0 => {
                                            format!("{} / {}", bytes(job.downloaded), bytes(total))
                                        }
                                        _ => format!(
                                            "已传输 {} · 总大小待确认",
                                            bytes(job.downloaded)
                                        ),
                                    },
                                );
                                if let Some(value) = measured {
                                    theme::muted(ui, format!("{:.1}%", value * 100.0));
                                }
                            });
                        }
                        if let Some(plan) = &job.plan {
                            theme::muted(
                                ui,
                                format!(
                                    "画质 ID {} · {} · MP4",
                                    plan.video.quality, plan.video.codec
                                ),
                            );
                        }
                        if completed {
                            if let Some(path) = &job.output {
                                theme::muted(ui, file_summary(path, job.downloaded));
                            }
                        }
                        errors(ui, job);
                        ui.horizontal_wrapped(|ui| {
                            let changing =
                                matches!(job.phase, JobPhase::Pausing | JobPhase::Cancelling);
                            ui.add_enabled_ui(
                                available && !pending.contains(&job.id) && !changing,
                                |ui| {
                                    if job.phase.is_active() {
                                        if theme::button(
                                            ui,
                                            "暂停",
                                            Icon::Pause,
                                            ButtonKind::Soft,
                                            reduced,
                                        )
                                        .clicked()
                                        {
                                            action = Some((job.id, JobAction::Pause));
                                        }
                                    } else if matches!(
                                        job.phase,
                                        JobPhase::Paused | JobPhase::Cancelled
                                    ) {
                                        if theme::button(
                                            ui,
                                            "继续下载",
                                            Icon::Play,
                                            ButtonKind::Soft,
                                            reduced,
                                        )
                                        .clicked()
                                        {
                                            action = Some((job.id, JobAction::Resume));
                                        }
                                    } else if (matches!(
                                        job.phase,
                                        JobPhase::Failed | JobPhase::NeedsAttention
                                    ) || job
                                        .components
                                        .iter()
                                        .any(|c| c.error.is_some()))
                                        && theme::button(
                                            ui,
                                            "重试",
                                            Icon::Refresh,
                                            ButtonKind::Soft,
                                            reduced,
                                        )
                                        .clicked()
                                    {
                                        action = Some((job.id, JobAction::Retry));
                                    }
                                    if !matches!(
                                        job.phase,
                                        JobPhase::Completed | JobPhase::Cancelled
                                    ) && theme::button(
                                        ui,
                                        "取消",
                                        Icon::Close,
                                        ButtonKind::Quiet,
                                        reduced,
                                    )
                                    .on_hover_text("停止任务并保留恢复数据，不删除已下载文件")
                                    .clicked()
                                    {
                                        action = Some((job.id, JobAction::Cancel));
                                    }
                                },
                            );
                            if pending.contains(&job.id) || changing {
                                theme::loading(ui, reduced, "等待核心保存状态");
                            }
                            if completed {
                                if let Some(path) = &job.output {
                                    if theme::button(
                                        ui,
                                        if self.copied == Some(job.id) {
                                            "已复制路径"
                                        } else {
                                            "复制文件路径"
                                        },
                                        Icon::Copy,
                                        ButtonKind::Soft,
                                        reduced,
                                    )
                                    .clicked()
                                    {
                                        ui.ctx().copy_text(path.display().to_string());
                                        self.copied = Some(job.id);
                                    }
                                }
                            }
                        });
                        if let Some(path) = &job.output {
                            let full = path.display().to_string();
                            let response = ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(&full).color(theme::muted_color()).size(12.0),
                                    )
                                    .truncate()
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_text(format!("{full}\n点击复制完整路径"));
                            if response.clicked() {
                                ui.ctx().copy_text(full);
                                self.copied = Some(job.id);
                            }
                        }
                        if job.skipped {
                            theme::muted(ui, "已验证已有文件，本次无需重复下载。");
                        }
                        if job.phase == JobPhase::Downloaded
                            && job
                                .plan
                                .as_ref()
                                .is_some_and(|p| p.merge == bili_core::MergePolicy::Manual)
                        {
                            theme::muted(
                                ui,
                                "此任务在 CLI 中选择了手动封装，请使用 bili merge --job 完成。",
                            );
                        }
                    });
                });
                ui.add_space(8.0);
            });
        }
        action
    }
}

fn file_summary(path: &Path, downloaded: u64) -> String {
    let size = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(downloaded);
    let ago = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(modified_ago)
        .unwrap_or_else(|| "时间未知".into());
    format!("{} · 完成于 {}", bytes(size), ago)
}

fn modified_ago(modified: SystemTime) -> String {
    let seconds = SystemTime::now()
        .duration_since(modified)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
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

fn errors(ui: &mut Ui, job: &JobSnapshot) {
    if let Some(error) = &job.error {
        ui.colored_label(theme::ERROR, backend::error_message(error))
            .on_hover_text(backend::error_code(error));
    }
    for component in &job.components {
        if let Some(error) = &component.error {
            ui.colored_label(
                theme::ERROR,
                format!("{}：{}", component.name, backend::error_message(error)),
            )
            .on_hover_text(backend::error_code(error));
        }
        if component.omissions > 0 {
            let response = ui.label(
                RichText::new(if component.name.contains("ass") {
                    format!(
                        "{}：有 {} 项未写入 ASS（可视区限流）",
                        component.name, component.omissions
                    )
                } else {
                    format!("{}：有 {} 项未能写入", component.name, component.omissions)
                })
                .size(12.0)
                .color(theme::muted_color()),
            );
            if component.name.contains("ass") {
                response.on_hover_text(
                    "ASS 只保留滚动 18 轨与顶部/底部各 4 轨，热门视频会省略超出的弹幕；完整弹幕见同名 .jsonl 与 .xml。",
                );
            } else {
                response.on_hover_text("完整内容见对应原始附件。");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_do_not_show_unfinished_jobs_as_completed() {
        assert!(!matches_filter(JobPhase::Remuxing, true, Filter::All));
        assert!(matches_filter(JobPhase::Completed, true, Filter::Attention));
        assert!(!matches_filter(JobPhase::Completed, false, Filter::All));
        assert!(matches_filter(
            JobPhase::NeedsAttention,
            false,
            Filter::Attention
        ));
        assert!(matches_filter(JobPhase::Paused, false, Filter::Paused));
    }
    #[test]
    fn progress_never_invents_a_total_or_exceeds_one() {
        assert_eq!(fraction(10, None), None);
        assert_eq!(fraction(10, Some(0)), None);
        assert_eq!(fraction(10, Some(20)), Some(0.5));
        assert_eq!(fraction(u64::MAX, Some(1)), Some(1.0));
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1536), "1.5 KiB");
    }
}
