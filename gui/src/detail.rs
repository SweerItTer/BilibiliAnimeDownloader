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
            let maximum = if matches!(
                self.category,
                SearchCategory::Bangumi | SearchCategory::Film
            ) {
                210.0
            } else {
                320.0
            };
            let width = (ui.available_width() * 0.36).clamp(120.0, maximum);
            let size = crate::covers::cover_size(self.category, width);
            if !covers.show(ui, bridge, self.item.cover.as_deref(), size, 4.0) {
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_filled(rect, 4.0, theme::soft());
            }
            egui::Frame::none()
                .fill(theme::card())
                .inner_margin(egui::vec2(8.0, 6.0))
                .show(ui, |ui| {
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
        });
        ui.add_space(16.0);
        let action = egui::Frame::none()
            .fill(theme::card())
            .inner_margin(egui::vec2(8.0, 6.0))
            .show(ui, |ui| {
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
                                    ui.checkbox(
                                        selected,
                                        format!("{:02}  {}", part.ordinal, part.title),
                                    );
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
                action
            })
            .inner;
        (back, action)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn render(
        ctx: &egui::Context,
        detail: &mut DetailState,
        covers: &mut CoverCache,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<Operation>) {
        let mut action = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                ctx.layer_painter(egui::LayerId::background()).rect_filled(
                    ctx.screen_rect(),
                    0.0,
                    egui::Color32::from_rgb(16, 20, 30),
                );
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        action = detail.show(ui, true, true, Path::new("."), covers, None).1;
                    });
            },
        );
        (output, action)
    }

    fn label_rect(output: &egui::FullOutput, label: &str) -> egui::Rect {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.galley.rect.translate(text.pos.to_vec2()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing label: {label}"))
    }

    fn click(
        ctx: &egui::Context,
        detail: &mut DetailState,
        covers: &mut CoverCache,
        point: egui::Pos2,
    ) -> (egui::FullOutput, Option<Operation>) {
        let _ = render(
            ctx,
            detail,
            covers,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        render(
            ctx,
            detail,
            covers,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
    }

    #[test]
    fn local_backing_keeps_details_readable_and_part_selection_deliberate() {
        let aid = bili_core::Aid::new(42).unwrap();
        let reference = bili_core::ContentRef::Video(aid);
        let parts: Vec<_> = ["First part", "Second part"]
            .into_iter()
            .enumerate()
            .map(|(i, title)| bili_core::Part {
                reference: PartRef {
                    content: reference.clone(),
                    aid,
                    cid: bili_core::Cid::new(420 + i as u64).unwrap(),
                    episode: None,
                },
                ordinal: i as u32 + 1,
                title: title.into(),
                duration_ms: Some(213_000),
                preview: false,
            })
            .collect();
        let first = parts[0].reference.clone();
        let mut detail = DetailState::new(MediaItem {
            reference,
            bvid: None,
            title: "Readable detail".into(),
            cover: None,
            duration_ms: Some(213_000),
            parts,
            source: None,
            description: Some("Readable description".into()),
            author: Some("Readable author".into()),
        });
        detail.category = SearchCategory::Video;
        let ctx = egui::Context::default();
        theme::apply_style(&ctx, false);
        let mut covers = CoverCache::default();
        let (output, _) = render(&ctx, &mut detail, &mut covers, vec![]);
        for label in [
            "Readable detail",
            "Readable description",
            "Readable author",
            "已选 0 / 2",
            "01  First part",
            "选择要下载的分集。",
        ] {
            let bounds = label_rect(&output, label);
            assert!(output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(rect)
                    if rect.fill == theme::card() && rect.fill.a() == 255 && rect.rect.contains_rect(bounds))
            }), "detail label has no opaque local backing: {label}");
        }
        let outside = egui::pos2(890.0, 690.0);
        assert!(!output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Rect(rect)
                if rect.fill == theme::card() && rect.rect.contains(outside))
        }));
        assert!(detail.selected.iter().all(|selected| !selected));
        let part = label_rect(&output, "01  First part").center();
        let (output, action) = click(&ctx, &mut detail, &mut covers, part);
        assert!(action.is_none());
        assert_eq!(detail.selected, [true, false]);
        let download = label_rect(&output, "下载").center();
        let (_, action) = click(&ctx, &mut detail, &mut covers, download);
        assert!(matches!(action,
            Some(Operation::Prepare { inputs, .. })
                if inputs.len() == 1 && inputs[0].parts.as_ref() == Some(&vec![first])));
    }

    #[test]
    fn titles_do_not_repeat() {
        assert_eq!(part_label("视频标题", "视频标题"), "视频标题");
        assert_eq!(part_label("视频", "P2 花絮"), "视频 - P2 花絮");
    }
}
