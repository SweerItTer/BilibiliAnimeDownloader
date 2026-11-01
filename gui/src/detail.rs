//! Content details and deliberate part selection. Configuration is shared with bulk browsing.
use crate::{
    backend::{Bridge, Operation, PrepareInput},
    covers::CoverCache,
    search, theme,
};
use bili_core::{MediaItem, PartRef, SearchCategory};
use egui::{RichText, Ui};
use std::path::Path;

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
#[derive(Clone)]
pub struct DetailState {
    pub item: MediaItem,
    selected: Vec<bool>,
    pub category: SearchCategory,
}
impl DetailState {
    pub fn new(item: MediaItem) -> Self {
        let selected = vec![item.parts.len() == 1; item.parts.len()];
        let category = item
            .source
            .as_ref()
            .and_then(|s| s.category)
            .unwrap_or(SearchCategory::All);
        Self {
            item,
            selected,
            category,
        }
    }
    pub fn acknowledge(&mut self, parts: &[PartRef]) {
        for (part, selected) in self.item.parts.iter().zip(&mut self.selected) {
            if parts.contains(&part.reference) {
                *selected = false;
            }
        }
    }
    pub fn show(
        &mut self,
        ui: &mut Ui,
        available: bool,
        _reduced: bool,
        _output: &Path,
        covers: &mut CoverCache,
        bridge: Option<&Bridge>,
    ) -> (bool, Option<Operation>) {
        let back = ui.button("返回搜索").clicked();
        ui.add_space(12.0);
        ui.horizontal_top(|ui| {
            let width = (ui.available_width() * 0.36).clamp(120.0, 320.0);
            let size = egui::vec2(width, width * 9.0 / 16.0);
            if !covers.show(ui, bridge, self.item.cover.as_deref(), size, 4.0) {
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_filled(rect, 4.0, theme::soft());
            }
            ui.vertical(|ui| {
                ui.label(RichText::new(&self.item.title).size(20.0).strong());
                theme::muted(
                    ui,
                    format!(
                        "{} · {}",
                        search::category_name(self.category),
                        search::duration(self.item.duration_ms)
                    ),
                );
                ui.label(search::reference_input(&self.item.reference));
                if let Some(author) = &self.item.author {
                    ui.label(author);
                }
                if let Some(description) = &self.item.description {
                    ui.label(description);
                }
            });
        });
        ui.add_space(16.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "已选 {} / {}",
                self.selected.iter().filter(|s| **s).count(),
                self.item.parts.len()
            ));
            if ui
                .add_enabled(available, egui::Button::new("全选"))
                .clicked()
            {
                self.selected.fill(true);
            }
            if ui
                .add_enabled(available, egui::Button::new("清空"))
                .clicked()
            {
                self.selected.fill(false);
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("detail-parts")
            .max_height(260.0)
            .show(ui, |ui| {
                for (part, selected) in self.item.parts.iter().zip(&mut self.selected) {
                    ui.add_enabled_ui(available, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(selected, format!("{:02}  {}", part.ordinal, part.title));
                            theme::muted(ui, search::duration(part.duration_ms));
                            if part.preview {
                                ui.colored_label(theme::ERROR, "仅试看");
                            }
                        });
                    });
                }
            });
        let parts: Vec<PartRef> = self
            .item
            .parts
            .iter()
            .zip(&self.selected)
            .filter(|(_, s)| **s)
            .map(|(p, _)| p.reference.clone())
            .collect();
        let action = if ui
            .add_enabled(available && !parts.is_empty(), egui::Button::new("下载"))
            .clicked()
        {
            Some(Operation::Prepare {
                refresh: false,
                inputs: vec![PrepareInput {
                    input: search::reference_input(&self.item.reference),
                    parts: Some(parts),
                    category: self.category,
                }],
                profile: None,
            })
        } else {
            None
        };
        if self.item.parts.len() > 1 && self.selected.iter().all(|s| !*s) {
            theme::muted(ui, "选择要下载的分集。");
        }
        (back, action)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn titles_do_not_repeat() {
        assert_eq!(part_label("视频标题", "视频标题"), "视频标题");
        assert_eq!(part_label("视频", "P2 花絮"), "视频 - P2 花絮");
    }
}
