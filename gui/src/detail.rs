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
        let width = ui.available_width();
        let maximum = if matches!(
            self.category,
            SearchCategory::Bangumi | SearchCategory::Film
        ) {
            180.0
        } else {
            320.0
        };
        let cover_width = (width * 0.30).clamp(96.0, maximum).min(width);
        let cover_size = crate::covers::cover_size(self.category, cover_width);
        let mut action = if width >= 360.0 {
            ui.horizontal_top(|ui| {
                self.cover(ui, cover_size, covers, bridge);
                self.summary(
                    ui,
                    (width - cover_width - ui.spacing().item_spacing.x).max(1.0),
                    available,
                )
            })
            .inner
        } else {
            self.cover(ui, cover_size, covers, bridge);
            ui.add_space(12.0);
            self.summary(ui, width, available)
        };
        if self.item.parts.len() > 1 {
            ui.add_space(16.0);
            action = self.parts(ui, available);
        }
        (back, action)
    }

    fn cover(
        &self,
        ui: &mut Ui,
        size: egui::Vec2,
        covers: &mut CoverCache,
        bridge: Option<&Bridge>,
    ) {
        if !covers.show(ui, bridge, self.item.cover.as_deref(), size, 4.0) {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.painter().rect_filled(rect, 4.0, theme::soft());
        }
    }

    fn summary(&self, ui: &mut Ui, width: f32, available: bool) -> Option<Operation> {
        egui::Frame::none()
            .fill(theme::card())
            .inner_margin(egui::vec2(8.0, 6.0))
            .show(ui, |ui| {
                ui.set_width((width - 16.0).max(1.0));
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
                        egui::ScrollArea::vertical()
                            .id_salt(("detail-description", &self.item.reference))
                            .max_height(100.0)
                            .show(ui, |ui| {
                                ui.label(description);
                            });
                    }
                    match self.item.parts.as_slice() {
                        [part] => {
                            ui.add_space(8.0);
                            if part.ordinal != 1 || part.title.trim() != self.item.title.trim() {
                                ui.label(format!("{:02}  {}", part.ordinal, part.title));
                            }
                            if part.preview {
                                ui.colored_label(theme::error_color(), "仅试看");
                            }
                            // A new explicit click is new intent. An acknowledged local
                            // selection is not permanent ownership of a file or task.
                            ui.add_enabled(available, egui::Button::new("下载"))
                                .clicked()
                                .then(|| self.prepare(vec![part.reference.clone()]))
                        }
                        [] => {
                            theme::muted(ui, "此内容暂无可下载分集。");
                            None
                        }
                        _ => None,
                    }
                })
                .inner
            })
            .inner
    }

    fn parts(&mut self, ui: &mut Ui, available: bool) -> Option<Operation> {
        let width = ui.available_width();
        egui::Frame::none()
            .fill(theme::card())
            .inner_margin(egui::vec2(8.0, 6.0))
            .show(ui, |ui| {
                ui.set_width((width - 16.0).max(1.0));
                let mut action = None;
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
                    if ui
                        .add_enabled(
                            available && self.selected.iter().any(|selected| *selected),
                            egui::Button::new("下载"),
                        )
                        .clicked()
                    {
                        action = Some(
                            self.prepare(
                                self.item
                                    .parts
                                    .iter()
                                    .zip(&self.selected)
                                    .filter(|(_, selected)| **selected)
                                    .map(|(part, _)| part.reference.clone())
                                    .collect(),
                            ),
                        );
                    }
                });
                if self.selected.iter().all(|selected| !*selected) {
                    theme::muted(ui, "选择要下载的分集。");
                }
                let height = ui.available_height().clamp(144.0, 360.0);
                egui::ScrollArea::vertical()
                    .id_salt(("detail-parts", &self.item.reference))
                    .auto_shrink([false, true])
                    .max_height(height)
                    .show(ui, |ui| {
                        let gap = 10.0;
                        let columns = ((ui.available_width() + gap) / (200.0 + gap))
                            .floor()
                            .max(1.0) as usize;
                        let tile_width = ((ui.available_width()
                            - gap * columns.saturating_sub(1) as f32)
                            / columns as f32)
                            .max(1.0);
                        egui::Grid::new(("detail-part-grid", &self.item.reference))
                            .num_columns(columns)
                            .spacing(egui::vec2(gap, gap))
                            .show(ui, |ui| {
                                for (index, (part, selected)) in
                                    self.item.parts.iter().zip(&mut self.selected).enumerate()
                                {
                                    ui.push_id(&part.reference, |ui| {
                                        ui.add_enabled_ui(available, |ui| {
                                            part_tile(ui, part, selected, tile_width);
                                        });
                                    });
                                    if (index + 1) % columns == 0 {
                                        ui.end_row();
                                    }
                                }
                            });
                    });
                action
            })
            .inner
    }

    fn prepare(&self, parts: Vec<PartRef>) -> Operation {
        Operation::Prepare {
            refresh: false,
            inputs: vec![PrepareInput {
                input: search::reference_input(&self.item.reference),
                parts: Some(parts),
                category: self.category,
            }],
            profile: None,
        }
    }
}

fn part_tile(ui: &mut Ui, part: &bili_core::Part, selected: &mut bool, width: f32) {
    let mut text = egui::text::LayoutJob::default();
    text.append(
        &format!("{:02}  {}", part.ordinal, part.title),
        0.0,
        egui::TextFormat {
            font_id: egui::TextStyle::Button.resolve(ui.style()),
            color: theme::ink(),
            ..Default::default()
        },
    );
    text.append(
        &format!("\n{}", search::duration(part.duration_ms)),
        0.0,
        egui::TextFormat {
            font_id: egui::TextStyle::Small.resolve(ui.style()),
            color: theme::muted_color(),
            ..Default::default()
        },
    );
    if part.preview {
        text.append(
            " · 仅试看",
            0.0,
            egui::TextFormat {
                font_id: egui::TextStyle::Small.resolve(ui.style()),
                color: theme::error_color(),
                ..Default::default()
            },
        );
    }
    text.wrap.max_rows = 3;
    text.wrap.break_anywhere = true;
    let tooltip = text.text.clone();
    egui::Frame::none()
        .fill(if *selected {
            theme::soft()
        } else {
            theme::card()
        })
        .stroke(egui::Stroke::new(1.0_f32, theme::border()))
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.add_sized(
                egui::vec2((width - 12.0).max(1.0), 52.0),
                egui::Checkbox::new(selected, text),
            )
            .on_hover_text(tooltip);
        });
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
        render_size(ctx, detail, covers, egui::vec2(900.0, 700.0), events)
    }

    fn render_size(
        ctx: &egui::Context,
        detail: &mut DetailState,
        covers: &mut CoverCache,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<Operation>) {
        let mut action = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
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
        click_size(ctx, detail, covers, egui::vec2(900.0, 700.0), point)
    }

    fn click_size(
        ctx: &egui::Context,
        detail: &mut DetailState,
        covers: &mut CoverCache,
        size: egui::Vec2,
        point: egui::Pos2,
    ) -> (egui::FullOutput, Option<Operation>) {
        let _ = render_size(
            ctx,
            detail,
            covers,
            size,
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
        render_size(
            ctx,
            detail,
            covers,
            size,
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
            "01  First part\n3:33",
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
        let part = label_rect(&output, "01  First part\n3:33").center();
        let (_, action) = click(&ctx, &mut detail, &mut covers, part);
        assert!(action.is_none());
        assert_eq!(detail.selected, [true, false]);
        // The header is painted before the grid updates selection. Let egui
        // register the now-enabled Download control before the next pointer-down.
        let (output, _) = render(&ctx, &mut detail, &mut covers, vec![]);
        label_rect(&output, "已选 1 / 2");
        let download = label_rect(&output, "下载").center();
        let (_, action) = click(&ctx, &mut detail, &mut covers, download);
        assert!(matches!(action,
            Some(Operation::Prepare { inputs, .. })
                if inputs.len() == 1 && inputs[0].parts.as_ref() == Some(&vec![first])));
    }

    fn season() -> DetailState {
        let aid = bili_core::Aid::new(42).unwrap();
        let reference = bili_core::ContentRef::Bangumi(bili_core::SeasonId::new(42).unwrap());
        let mut detail = DetailState::new(MediaItem {
            reference: reference.clone(),
            bvid: None,
            title: "Season details".into(),
            cover: None,
            duration_ms: None,
            parts: (3..27)
                .map(|ordinal| bili_core::Part {
                    reference: PartRef {
                        content: reference.clone(),
                        aid,
                        cid: bili_core::Cid::new(420 + u64::from(ordinal)).unwrap(),
                        episode: Some(bili_core::EpisodeId::new(100 + u64::from(ordinal)).unwrap()),
                    },
                    ordinal,
                    title: format!("EP{ordinal}"),
                    duration_ms: Some(180_000),
                    preview: false,
                })
                .collect(),
            source: None,
            description: Some("A real synopsis occupies the remaining summary width.".into()),
            author: None,
        });
        detail.category = SearchCategory::Bangumi;
        detail
    }

    #[test]
    fn episode_tiles_fill_responsive_rows_and_prepare_only_the_deliberate_original_parts() {
        for size in [
            egui::vec2(1180.0, 800.0),
            egui::vec2(900.0, 640.0),
            egui::vec2(360.0, 760.0),
        ] {
            let ctx = egui::Context::default();
            theme::apply_style(&ctx, false);
            let mut detail = season();
            let first = detail.item.parts[0].reference.clone();
            let second = detail.item.parts[1].reference.clone();
            let mut covers = CoverCache::default();
            let _ = render_size(&ctx, &mut detail, &mut covers, size, vec![]);
            let (output, _) = render_size(&ctx, &mut detail, &mut covers, size, vec![]);
            let first_tile = label_rect(&output, "03  EP3\n3:00");
            let second_tile = label_rect(&output, "04  EP4\n3:00");
            if size.x >= 900.0 {
                assert!(
                    second_tile.left() > first_tile.right(),
                    "wide episode grid must use more than one column: {size:?}"
                );
                assert!((first_tile.top() - second_tile.top()).abs() < 1.0);
            } else {
                assert!(second_tile.top() > first_tile.bottom());
            }
            assert!(
                output.shapes.iter().any(|shape| {
                    matches!(&shape.shape, egui::Shape::Rect(rect)
                    if rect.fill == theme::card()
                        && rect.rect.contains_rect(first_tile)
                        && rect.rect.left() <= 1.0
                        && rect.rect.right() >= size.x - 1.0)
                }),
                "episode backing must fill the page width at {size:?}"
            );
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            for text in ["Season details", "已选 0 / 24", "全选", "清空", "下载"] {
                assert!(
                    screen.contains_rect(label_rect(&output, text)),
                    "{size:?}: {text}"
                );
            }
            let (_, action) =
                click_size(&ctx, &mut detail, &mut covers, size, second_tile.center());
            assert!(action.is_none());
            assert!(detail.selected[1]);
            assert_eq!(
                detail.selected.iter().filter(|selected| **selected).count(),
                1
            );
            let (output, _) = render_size(&ctx, &mut detail, &mut covers, size, vec![]);
            let (_, action) = click_size(
                &ctx,
                &mut detail,
                &mut covers,
                size,
                label_rect(&output, "下载").center(),
            );
            assert!(matches!(action,
                Some(Operation::Prepare { refresh: false, inputs, .. })
                    if inputs.len() == 1 && inputs[0].parts.as_ref() == Some(&vec![second])));
            let (output, _) = render_size(&ctx, &mut detail, &mut covers, size, vec![]);
            let _ = click_size(
                &ctx,
                &mut detail,
                &mut covers,
                size,
                label_rect(&output, "全选").center(),
            );
            assert!(detail.selected.iter().all(|selected| *selected));
            detail.acknowledge(std::slice::from_ref(&first));
            assert!(!detail.selected[0]);
            assert_eq!(
                detail.selected.iter().filter(|selected| **selected).count(),
                23
            );
            let (output, _) = render_size(&ctx, &mut detail, &mut covers, size, vec![]);
            let _ = click_size(
                &ctx,
                &mut detail,
                &mut covers,
                size,
                label_rect(&output, "清空").center(),
            );
            assert!(detail.selected.iter().all(|selected| !*selected));
        }
    }

    #[test]
    fn episode_scrolling_preserves_visible_selection_controls_and_original_final_ordinal() {
        let size = egui::vec2(900.0, 640.0);
        let ctx = egui::Context::default();
        theme::apply_style(&ctx, false);
        let mut detail = season();
        let last = detail.item.parts.last().unwrap().reference.clone();
        let mut covers = CoverCache::default();
        let _ = render_size(&ctx, &mut detail, &mut covers, size, vec![]);
        let (output, _) = render_size(&ctx, &mut detail, &mut covers, size, vec![]);
        let controls = label_rect(&output, "下载");
        let first_tile = label_rect(&output, "03  EP3\n3:00");
        let _ = render_size(
            &ctx,
            &mut detail,
            &mut covers,
            size,
            vec![
                egui::Event::PointerMoved(first_tile.center()),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -1200.0),
                    modifiers: Default::default(),
                },
            ],
        );
        for _ in 0..30 {
            let _ = render_size(&ctx, &mut detail, &mut covers, size, vec![]);
        }
        let (output, _) = render_size(&ctx, &mut detail, &mut covers, size, vec![]);
        let final_tile = label_rect(&output, "26  EP26\n3:00");
        assert_eq!(label_rect(&output, "下载"), controls);
        assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(final_tile));
        let _ = click_size(&ctx, &mut detail, &mut covers, size, final_tile.center());
        assert!(detail.selected.last().copied().unwrap());
        let (output, _) = render_size(&ctx, &mut detail, &mut covers, size, vec![]);
        let (_, action) = click_size(
            &ctx,
            &mut detail,
            &mut covers,
            size,
            label_rect(&output, "下载").center(),
        );
        assert!(matches!(action,
            Some(Operation::Prepare { inputs, .. })
                if inputs[0].parts.as_ref() == Some(&vec![last])));
    }

    #[test]
    fn unique_and_explicit_parts_have_one_download_action_that_survives_acknowledgement_and_reentry(
    ) {
        for category in [SearchCategory::Video, SearchCategory::Bangumi] {
            let mut item = season().item;
            let mut part = item.parts.remove(if category == SearchCategory::Video {
                0
            } else {
                9
            });
            if category == SearchCategory::Video {
                item.reference = bili_core::ContentRef::Video(part.reference.aid);
                part.reference.episode = None;
                part.title = "P3".into();
            } else {
                item.reference = bili_core::ContentRef::Episode(part.reference.episode.unwrap());
            }
            part.reference.content = item.reference.clone();
            item.parts = vec![part.clone()];
            let mut detail = DetailState::new(item);
            detail.category = category;
            let ctx = egui::Context::default();
            theme::apply_style(&ctx, false);
            let mut covers = CoverCache::default();
            let (output, _) = render(&ctx, &mut detail, &mut covers, vec![]);
            let expected = if category == SearchCategory::Video {
                "03  P3"
            } else {
                "12  EP12"
            };
            label_rect(&output, expected);
            for label in ["全选", "清空", "已选 1 / 1", "选择要下载的分集。"] {
                assert!(!output.shapes.iter().any(|shape| {
                    matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == label)
                }), "single part must not expose bulk controls: {label}");
            }
            let (_, action) = click(
                &ctx,
                &mut detail,
                &mut covers,
                label_rect(&output, "下载").center(),
            );
            assert!(matches!(action,
                Some(Operation::Prepare { refresh: false, inputs, .. })
                    if inputs[0].parts.as_ref() == Some(&vec![part.reference.clone()])));
            detail.acknowledge(std::slice::from_ref(&part.reference));
            assert_eq!(detail.selected, [false]);
            let mut reopened = detail.clone();
            let (output, _) = render(&ctx, &mut reopened, &mut covers, vec![]);
            let (_, action) = click(
                &ctx,
                &mut reopened,
                &mut covers,
                label_rect(&output, "下载").center(),
            );
            assert!(matches!(action,
                Some(Operation::Prepare { inputs, .. })
                    if inputs[0].parts.as_ref() == Some(&vec![part.reference])));
            assert_eq!(
                reopened.selected,
                [false],
                "reentry must not silently reselect an acknowledged part"
            );
        }
    }

    #[test]
    fn titles_do_not_repeat() {
        assert_eq!(part_label("视频标题", "视频标题"), "视频标题");
        assert_eq!(part_label("视频", "P2 花絮"), "视频 - P2 花絮");
    }
}
