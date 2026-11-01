//! Native part selection. Quality negotiation and safe filenames stay in core.
use crate::{
    backend::{Bridge, Operation},
    covers::CoverCache,
    search,
    theme::{self, ButtonKind, Icon},
};
use bili_core::{DownloadSelection, MediaItem, MergePolicy, SidecarPolicy, SubtitleSelection};
use egui::{RichText, Ui};
use std::path::Path;

/// Keeps a single copy when the part title repeats the video title.
pub fn part_label(video: &str, part: &str) -> String {
    let video = video.trim();
    let part = part.trim();
    if part.is_empty() || part == video || video.contains(part) {
        video.to_owned()
    } else if part.contains(video) {
        part.to_owned()
    } else {
        format!("{video} - {part}")
    }
}

pub struct DetailState {
    pub item: MediaItem,
    selected: Vec<bool>,
    pub subtitles: bool,
    pub danmaku: bool,
}

impl DetailState {
    pub fn new(item: MediaItem) -> Self {
        let selected = vec![true; item.parts.len()];
        Self {
            item,
            selected,
            subtitles: true,
            danmaku: true,
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        available: bool,
        reduced: bool,
        output: &Path,
        covers: &mut CoverCache,
        bridge: Option<&Bridge>,
    ) -> (bool, Option<Operation>) {
        let mut back = false;
        let mut action = None;
        if theme::button(ui, "返回搜索", Icon::Back, ButtonKind::Quiet, reduced).clicked() {
            back = true;
        }
        ui.add_space(8.0);
        theme::card_frame().show(ui, |ui| {
            theme::inset(theme::soft()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let size = egui::vec2(288.0, 162.0);
                    if !covers.show(ui, bridge, self.item.cover.as_deref(), size, 18.0) {
                        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                        ui.painter().rect_filled(rect, 18.0, theme::background());
                        theme::icon(
                            ui.painter(),
                            egui::Rect::from_center_size(rect.center(), egui::vec2(42.0, 42.0)),
                            Icon::Play,
                            theme::primary(),
                        );
                    }
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.add_space(16.0);
                        ui.add(
                            egui::Label::new(RichText::new(&self.item.title).size(24.0).strong())
                                .truncate(),
                        )
                        .on_hover_text(&self.item.title);
                        theme::muted(
                            ui,
                            format!(
                                "{}  ·  {} 个分集 / 分 P",
                                search::reference_input(&self.item.reference),
                                self.item.parts.len()
                            ),
                        );
                    });
                });
            });
            theme::inset(theme::card()).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("选择内容").size(18.0).strong());
                    theme::muted(
                        ui,
                        format!(
                            "已选 {} / {}",
                            self.selected.iter().filter(|&&v| v).count(),
                            self.selected.len()
                        ),
                    );
                    ui.add_enabled_ui(available, |ui| {
                        if theme::button(ui, "全选", Icon::Check, ButtonKind::Quiet, reduced)
                            .clicked()
                        {
                            self.selected.fill(true);
                        }
                        if theme::button(ui, "清空", Icon::Close, ButtonKind::Quiet, reduced)
                            .clicked()
                        {
                            self.selected.fill(false);
                        }
                    });
                });
                ui.add_space(6.0);
                ui.add_enabled_ui(available, |ui| {
                    // Core ordinals are preserved; episode links can resolve to
                    // a single part with an ordinal other than one.
                    for (index, part) in self.item.parts.iter().enumerate() {
                        ui.push_id(index, |ui| {
                            theme::inset(if self.selected[index] {
                                theme::soft()
                            } else {
                                theme::background()
                            })
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal_wrapped(|ui| {
                                    ui.checkbox(
                                        &mut self.selected[index],
                                        RichText::new(format!(
                                            "{:02}   {}",
                                            part.ordinal, part.title
                                        ))
                                        .strong(),
                                    );
                                    theme::muted(ui, search::duration(part.duration_ms));
                                    if part.preview {
                                        ui.label(RichText::new("仅试看").color(theme::ERROR));
                                    }
                                });
                            });
                        });
                    }
                });
                if self.item.parts.is_empty() {
                    theme::muted(ui, "当前内容没有可下载的分集。");
                }
                ui.add_space(12.0);
                ui.label(RichText::new("下载选项").size(18.0).strong());
                theme::muted(ui, "自动选择账号可获得的最高画质；下载后自动封装为 MP4。");
                ui.add_enabled_ui(available, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.subtitles, "下载全部可用字幕");
                        ui.checkbox(&mut self.danmaku, "下载当前弹幕");
                    });
                });
                ui.label(
                    RichText::new("字幕与弹幕单独保存；权限与可用性以站点返回为准。")
                        .size(12.0)
                        .color(theme::muted_color()),
                );
                ui.add_space(8.0);
                theme::inset(theme::background()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new("保存到")
                            .size(12.0)
                            .color(theme::muted_color()),
                    );
                    ui.label(output.display().to_string());
                });
                ui.add_space(10.0);
                let count = self.selected.iter().filter(|&&v| v).count();
                ui.add_enabled_ui(available && count > 0, |ui| {
                    if theme::button(
                        ui,
                        &format!("添加 {count} 项到下载队列"),
                        Icon::Download,
                        ButtonKind::Primary,
                        reduced,
                    )
                    .clicked()
                    {
                        let selections = self
                            .item
                            .parts
                            .iter()
                            .zip(&self.selected)
                            .filter(|(_, selected)| **selected)
                            .map(|(part, _)| DownloadSelection {
                                part: part.reference.clone(),
                                title: part_label(&self.item.title, &part.title),
                                quality: None,
                                video_representation: None,
                                audio_representation: None,
                                codec: None,
                                dynamic_range: None,
                                output: None,
                                merge: MergePolicy::Automatic,
                                sidecars: SidecarPolicy {
                                    subtitles: if self.subtitles {
                                        SubtitleSelection::All
                                    } else {
                                        SubtitleSelection::None
                                    },
                                    danmaku: self.danmaku,
                                },
                                keep_temp: false,
                            })
                            .collect();
                        action = Some(Operation::Enqueue {
                            selections,
                            output_dir: output.to_owned(),
                        });
                    }
                });
            });
        });
        (back, action)
    }
}

#[cfg(test)]
mod tests {
    use super::part_label;

    #[test]
    fn repeated_part_titles_are_collapsed() {
        assert_eq!(
            part_label("视频标题", "视频标题"),
            "视频标题",
            "identical titles"
        );
        assert_eq!(
            part_label(
                "【官方 MV】Never Gonna Give You Up - Rick Astley",
                "Never Gonna Give You Up - Rick Astley"
            ),
            "【官方 MV】Never Gonna Give You Up - Rick Astley",
            "part title already contained in the video title"
        );
        assert_eq!(
            part_label("第 1 集 开端", "开端"),
            "第 1 集 开端",
            "short part title inside the video title"
        );
        assert_eq!(part_label("视频", "P2 花絮"), "视频 - P2 花絮");
        assert_eq!(part_label("视频", "  "), "视频");
    }
}
