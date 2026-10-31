//! User-chosen background image: bounded decode, crop, blur, and native painting.
use crate::prefs::{BackgroundScope, GuiPrefs};
use egui::{pos2, vec2, Color32, Rect, Sense, TextureHandle, TextureOptions, Vec2};
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PIXELS: u64 = 40_000_000;
const BASE_WIDTH: u32 = 1600;
const BLUR_WIDTH: u32 = 720;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CropEvent {
    None,
    Applied,
    Cancelled,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Move,
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    crop: [i32; 4],
    blur: i32,
}

pub struct Background {
    path: Option<PathBuf>,
    source: Option<image::RgbaImage>,
    texture: Option<TextureHandle>,
    tex_size: Vec2,
    key: Option<Key>,
    error: Option<String>,
    pub crop_open: bool,
    draft: [f32; 4],
    preview: Option<(TextureHandle, Vec2)>,
    drag: Option<Drag>,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            path: None,
            source: None,
            texture: None,
            tex_size: Vec2::ZERO,
            key: None,
            error: None,
            crop_open: false,
            draft: [0.0, 0.0, 1.0, 1.0],
            preview: None,
            drag: None,
        }
    }
}

fn load_source(path: &Path) -> Result<image::RgbaImage, String> {
    let metadata = std::fs::metadata(path).map_err(|_| "无法读取图片文件。".to_owned())?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err("图片不存在，或文件超过 64 MiB。".to_owned());
    }
    let image = image::open(path)
        .map_err(|_| "无法解码该图片，请使用 PNG / JPEG / WebP。".to_owned())?
        .to_rgba8();
    let (w, h) = image.dimensions();
    if w < 64 || h < 64 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err("图片尺寸不受支持（至少 64×64，最多 4000 万像素）。".to_owned());
    }
    Ok(image)
}

fn crop_rect(size: (u32, u32), crop: [f32; 4]) -> (u32, u32, u32, u32) {
    let w = size.0 as f32;
    let h = size.1 as f32;
    let x = (crop[0].clamp(0.0, 1.0) * w).round() as u32;
    let y = (crop[1].clamp(0.0, 1.0) * h).round() as u32;
    let cw = ((crop[2].clamp(0.02, 1.0) * w).round() as u32).max(8);
    let ch = ((crop[3].clamp(0.02, 1.0) * h).round() as u32).max(8);
    (
        x.min(size.0 - 1),
        y.min(size.1 - 1),
        cw.min(size.0 - x),
        ch.min(size.1 - y),
    )
}

fn cover_rect(target: Rect, image: Vec2) -> Rect {
    if image.x <= 0.0 || image.y <= 0.0 {
        return target;
    }
    let scale = (target.width() / image.x).max(target.height() / image.y);
    let size = image * scale;
    Rect::from_center_size(target.center(), size)
}

impl Background {
    pub fn has_image(&self) -> bool {
        self.texture.is_some()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn draft(&self) -> [f32; 4] {
        self.draft
    }

    /// True when the image should show behind the given column.
    pub fn visible_for(&self, prefs: &GuiPrefs, sidebar: bool) -> bool {
        self.has_image() && (sidebar && prefs.background_scope == BackgroundScope::Full || !sidebar)
    }

    pub fn sync(&mut self, ctx: &egui::Context, prefs: &GuiPrefs) {
        if prefs.background.as_deref() != self.path.as_deref() {
            self.path = prefs.background.clone();
            self.texture = None;
            self.key = None;
            self.source = None;
            self.preview = None;
            self.error = None;
            self.draft = prefs.crop;
            if let Some(path) = &self.path {
                match load_source(path) {
                    Ok(source) => {
                        self.source = Some(source);
                        self.refresh_preview(ctx);
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        if self.source.is_none() {
            return;
        }
        let blur = prefs.blur.round() as i32;
        let key = Key {
            path,
            crop: prefs.crop.map(|v| (v * 1000.0).round() as i32),
            blur,
        };
        if self.key.as_ref() != Some(&key) {
            self.rebuild(ctx, prefs.crop, prefs.blur);
            self.key = Some(key);
        }
    }

    fn rebuild(&mut self, ctx: &egui::Context, crop: [f32; 4], blur: f32) {
        let Some(source) = &self.source else {
            return;
        };
        let (x, y, w, h) = crop_rect(source.dimensions(), crop);
        let cropped = image::imageops::crop_imm(source, x, y, w, h).to_image();
        let limit = if blur > 0.5 { BLUR_WIDTH } else { BASE_WIDTH };
        let mut image = if cropped.width() > limit {
            let height = (cropped.height() * limit / cropped.width().max(1)).max(1);
            image::imageops::resize(
                &cropped,
                limit,
                height,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            cropped
        };
        if blur > 0.5 {
            let sigma = (blur / 100.0) * (image.width() as f32 / 24.0);
            if sigma > 0.2 {
                image = image::imageops::blur(&image, sigma);
            }
        }
        self.tex_size = vec2(image.width() as f32, image.height() as f32);
        self.texture = Some(ctx.load_texture(
            "gui-background",
            egui::ColorImage::from_rgba_unmultiplied(
                [image.width() as usize, image.height() as usize],
                image.as_raw(),
            ),
            TextureOptions::LINEAR,
        ));
    }

    fn refresh_preview(&mut self, ctx: &egui::Context) {
        let Some(source) = &self.source else {
            return;
        };
        let limit = 900;
        let image = if source.width() > limit {
            let height = (source.height() * limit / source.width().max(1)).max(1);
            image::imageops::resize(source, limit, height, image::imageops::FilterType::Triangle)
        } else {
            source.clone()
        };
        self.preview = Some((
            ctx.load_texture(
                "gui-background-preview",
                egui::ColorImage::from_rgba_unmultiplied(
                    [image.width() as usize, image.height() as usize],
                    image.as_raw(),
                ),
                TextureOptions::LINEAR,
            ),
            vec2(image.width() as f32, image.height() as f32),
        ));
    }

    pub fn paint(&self, ctx: &egui::Context, clip: Rect, prefs: &GuiPrefs) {
        let Some(texture) = &self.texture else {
            return;
        };
        let visibility = (prefs.opacity / 100.0).clamp(0.0, 1.0);
        if visibility <= 0.0 || clip.width() <= 1.0 || clip.height() <= 1.0 {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::background());
        let rect = cover_rect(clip, self.tex_size);
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::from_white_alpha((visibility * 255.0).round() as u8),
        );
    }

    pub fn open_crop(&mut self, ctx: &egui::Context, prefs: &GuiPrefs) {
        if self.source.is_none() {
            return;
        }
        self.draft = prefs.crop;
        self.drag = None;
        self.refresh_preview(ctx);
        self.crop_open = true;
    }

    /// Draws the crop editor. Returns the outcome chosen by the user.
    pub fn show_crop(&mut self, ui: &mut egui::Ui, prefs: &GuiPrefs) -> CropEvent {
        let Some((texture, source_size)) = self.preview.clone() else {
            self.crop_open = false;
            return CropEvent::Cancelled;
        };
        let mut event = CropEvent::None;
        crate::theme::card_frame().show(ui, |ui| {
            crate::theme::inset(crate::theme::card()).show(ui, |ui| {
                ui.label(
                    egui::RichText::new("拖动方框移动，拖角调整范围")
                        .size(13.0)
                        .color(crate::theme::muted_color()),
                );
                ui.add_space(8.0);
                let max = vec2(560.0, 380.0);
                let scale = (max.x / source_size.x).min(max.y / source_size.y).min(1.0);
                let size = source_size * scale;
                let pad = ((ui.available_width() - size.x - 28.0) / 2.0).max(0.0);
                let (outer, response) = ui
                    .horizontal(|ui| {
                        ui.add_space(pad);
                        ui.allocate_exact_size(size + vec2(28.0, 28.0), Sense::click_and_drag())
                    })
                    .inner;
                let rect = Rect::from_center_size(outer.center(), size);
                let painter = ui.painter_at(rect);
                painter.image(
                    texture.id(),
                    rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                let mut crop = self.draft;
                if let Some(pos) = response.interact_pointer_pos() {
                    let min = rect.min;
                    let crop_rect = Rect::from_min_max(
                        pos2(
                            min.x + crop[0] * rect.width(),
                            min.y + crop[1] * rect.height(),
                        ),
                        pos2(
                            min.x + (crop[0] + crop[2]) * rect.width(),
                            min.y + (crop[1] + crop[3]) * rect.height(),
                        ),
                    );
                    if response.drag_started() {
                        let origin = ui.ctx().input(|i| i.pointer.press_origin()).unwrap_or(pos);
                        self.drag = Some(handle_at(crop_rect, origin));
                    }
                    if response.dragged() {
                        let delta = response.drag_delta();
                        let dx = delta.x / rect.width();
                        let dy = delta.y / rect.height();
                        match self.drag.unwrap_or(Drag::Move) {
                            Drag::Move => {
                                crop[0] = (crop[0] + dx).clamp(0.0, 1.0 - crop[2]);
                                crop[1] = (crop[1] + dy).clamp(0.0, 1.0 - crop[3]);
                            }
                            Drag::NorthWest => {
                                let nx = (crop[0] + dx).clamp(0.0, crop[0] + crop[2] - 0.05);
                                let ny = (crop[1] + dy).clamp(0.0, crop[1] + crop[3] - 0.05);
                                crop[2] += crop[0] - nx;
                                crop[3] += crop[1] - ny;
                                crop[0] = nx;
                                crop[1] = ny;
                            }
                            Drag::NorthEast => {
                                let ny = (crop[1] + dy).clamp(0.0, crop[1] + crop[3] - 0.05);
                                crop[3] += crop[1] - ny;
                                crop[1] = ny;
                                crop[2] = (crop[2] + dx).clamp(0.05, 1.0 - crop[0]);
                            }
                            Drag::SouthWest => {
                                let nx = (crop[0] + dx).clamp(0.0, crop[0] + crop[2] - 0.05);
                                crop[2] += crop[0] - nx;
                                crop[0] = nx;
                                crop[3] = (crop[3] + dy).clamp(0.05, 1.0 - crop[1]);
                            }
                            Drag::SouthEast => {
                                crop[2] = (crop[2] + dx).clamp(0.05, 1.0 - crop[0]);
                                crop[3] = (crop[3] + dy).clamp(0.05, 1.0 - crop[1]);
                            }
                        }
                        self.draft = crop;
                    }
                }
                let crop_rect = Rect::from_min_max(
                    pos2(
                        rect.min.x + self.draft[0] * rect.width(),
                        rect.min.y + self.draft[1] * rect.height(),
                    ),
                    pos2(
                        rect.min.x + (self.draft[0] + self.draft[2]) * rect.width(),
                        rect.min.y + (self.draft[1] + self.draft[3]) * rect.height(),
                    ),
                );
                let dim = Color32::from_black_alpha(110);
                let top = Rect::from_min_max(rect.min, pos2(rect.max.x, crop_rect.min.y));
                let bottom = Rect::from_min_max(pos2(rect.min.x, crop_rect.max.y), rect.max);
                let left = Rect::from_min_max(
                    pos2(rect.min.x, crop_rect.min.y),
                    pos2(crop_rect.min.x, crop_rect.max.y),
                );
                let right = Rect::from_min_max(
                    pos2(crop_rect.max.x, crop_rect.min.y),
                    pos2(rect.max.x, crop_rect.max.y),
                );
                for area in [top, bottom, left, right] {
                    painter.rect_filled(area, 0.0, dim);
                }
                painter.rect_stroke(crop_rect, 6.0, egui::Stroke::new(2.0_f32, Color32::WHITE));
                for corner in [
                    crop_rect.left_top(),
                    crop_rect.right_top(),
                    crop_rect.left_bottom(),
                    crop_rect.right_bottom(),
                ] {
                    painter.rect_filled(
                        Rect::from_center_size(corner, vec2(12.0, 12.0)),
                        3.0,
                        Color32::WHITE,
                    );
                }
                ui.add_space(10.0);
                let mut reset = false;
                let mut cancel = false;
                let mut apply = false;
                ui.horizontal(|ui| {
                    if crate::theme::button(
                        ui,
                        "重置",
                        crate::theme::Icon::Refresh,
                        crate::theme::ButtonKind::Quiet,
                        prefs.reduced_motion,
                    )
                    .clicked()
                    {
                        reset = true;
                    }
                    ui.add_space(18.0);
                    if crate::theme::button(
                        ui,
                        "取消",
                        crate::theme::Icon::Close,
                        crate::theme::ButtonKind::Quiet,
                        prefs.reduced_motion,
                    )
                    .clicked()
                    {
                        cancel = true;
                    }
                    if crate::theme::button(
                        ui,
                        "应用裁剪",
                        crate::theme::Icon::Check,
                        crate::theme::ButtonKind::Primary,
                        prefs.reduced_motion,
                    )
                    .clicked()
                    {
                        apply = true;
                    }
                });
                if reset {
                    self.draft = [0.0, 0.0, 1.0, 1.0];
                }
                if cancel {
                    self.draft = prefs.crop;
                    self.crop_open = false;
                    event = CropEvent::Cancelled;
                }
                if apply {
                    self.crop_open = false;
                    event = CropEvent::Applied;
                }
            });
        });
        event
    }
}

fn handle_at(rect: Rect, pos: egui::Pos2) -> Drag {
    let near = |p: egui::Pos2| pos.distance(p) <= 20.0;
    if near(rect.left_top()) {
        Drag::NorthWest
    } else if near(rect.right_top()) {
        Drag::NorthEast
    } else if near(rect.left_bottom()) {
        Drag::SouthWest
    } else if near(rect.right_bottom()) {
        Drag::SouthEast
    } else {
        Drag::Move
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_rect_stays_inside_the_image() {
        assert_eq!(crop_rect((100, 50), [0.5, 0.5, 0.5, 0.5]), (50, 25, 50, 25));
        assert_eq!(crop_rect((100, 50), [0.98, 0.0, 1.0, 1.0]), (98, 0, 2, 50));
    }

    #[test]
    fn cover_fit_keeps_aspect_and_covers_target() {
        let target = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 100.0));
        let wide = cover_rect(target, vec2(400.0, 100.0));
        assert!(wide.width() >= target.width() - 0.1);
        assert!(wide.height() >= target.height() - 0.1);
        let tall = cover_rect(target, vec2(100.0, 400.0));
        assert!(tall.width() >= target.width() - 0.1);
        assert!(tall.height() >= target.height() - 0.1);
    }

    #[test]
    fn corner_handles_win_near_the_edge() {
        let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(100.0, 80.0));
        assert!(matches!(handle_at(rect, pos2(12.0, 11.0)), Drag::NorthWest));
        assert!(matches!(
            handle_at(rect, pos2(108.0, 88.0)),
            Drag::SouthEast
        ));
        assert!(matches!(handle_at(rect, pos2(60.0, 50.0)), Drag::Move));
    }
}
