//! Read-only cover thumbnails fetched through core's validated cover path.
use crate::backend::{Bridge, Command};
use std::collections::{HashMap, HashSet};

/// Series/movie posters use a portrait slot; individual videos use 16:9.
pub fn cover_size(category: bili_core::SearchCategory, width: f32) -> egui::Vec2 {
    let height = match category {
        bili_core::SearchCategory::Bangumi | bili_core::SearchCategory::Film => width * 4.0 / 3.0,
        _ => width * 9.0 / 16.0,
    };
    egui::vec2(width, height)
}

fn contained_rect(slot: egui::Rect, source: egui::Vec2) -> egui::Rect {
    let scale = (slot.width() / source.x).min(slot.height() / source.y);
    egui::Align2::CENTER_CENTER.align_size_within_rect(source * scale, slot)
}

#[derive(Default)]
pub struct CoverCache {
    textures: HashMap<String, egui::TextureHandle>,
    requested: HashSet<String>,
    failed: HashSet<String>,
}

impl CoverCache {
    pub fn ensure(&mut self, bridge: Option<&Bridge>, url: &str) {
        if url.is_empty()
            || self.textures.contains_key(url)
            || self.requested.contains(url)
            || self.failed.contains(url)
        {
            return;
        }
        let Some(bridge) = bridge else {
            return;
        };
        if bridge.send(Command::Cover(url.to_owned())).is_ok() {
            self.requested.insert(url.to_owned());
        }
    }

    pub fn insert(&mut self, ctx: &egui::Context, url: String, image: Option<egui::ColorImage>) {
        self.requested.remove(&url);
        match image {
            Some(image) => {
                let texture =
                    ctx.load_texture(format!("cover:{url}"), image, egui::TextureOptions::LINEAR);
                self.textures.insert(url, texture);
            }
            None => {
                self.failed.insert(url);
            }
        }
    }

    pub fn texture(&self, url: &str) -> Option<egui::TextureId> {
        self.textures.get(url).map(|texture| texture.id())
    }

    /// Draws the thumbnail when available and starts a fetch otherwise.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        bridge: Option<&Bridge>,
        url: Option<&str>,
        size: egui::Vec2,
        rounding: f32,
    ) -> bool {
        let Some(url) = url.filter(|url| !url.is_empty()) else {
            return false;
        };
        self.ensure(bridge, url);
        let Some(texture) = self.textures.get(url) else {
            return false;
        };
        let (slot, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let source = texture.size_vec2();
        let rect = contained_rect(slot, source);
        egui::Image::new(texture)
            .rounding(rounding)
            .paint_at(ui, rect);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_cover_is_cached_and_failures_are_terminal() {
        let ctx = egui::Context::default();
        let mut cache = CoverCache::default();
        cache.insert(
            &ctx,
            "https://i0.hdslb.com/a.jpg".into(),
            Some(egui::ColorImage::new([2, 2], egui::Color32::WHITE)),
        );
        assert!(cache.texture("https://i0.hdslb.com/a.jpg").is_some());
        cache.insert(&ctx, "https://i0.hdslb.com/b.jpg".into(), None);
        assert!(cache.texture("https://i0.hdslb.com/b.jpg").is_none());
    }
    #[test]
    fn rendered_covers_keep_source_aspect_and_do_not_move_the_next_widget() {
        for source in [[300, 420], [420, 236]] {
            for category in [
                bili_core::SearchCategory::Video,
                bili_core::SearchCategory::Bangumi,
                bili_core::SearchCategory::Film,
            ] {
                let ctx = egui::Context::default();
                let mut cache = CoverCache::default();
                let url = "fixture:cover";
                cache.insert(
                    &ctx,
                    url.into(),
                    Some(egui::ColorImage::new(source, egui::Color32::LIGHT_BLUE)),
                );
                let texture = cache.texture(url).unwrap();
                let size = cover_size(category, 240.0);
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 600.0),
                    )),
                    ..Default::default()
                };
                let mut next_cached = egui::Rect::NOTHING;
                let output = ctx.run(raw.clone(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        assert!(cache.show(ui, None, Some(url), size, 4.0));
                        next_cached = ui.label("next").rect;
                    });
                });
                let painted = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect) if rect.fill_texture_id == texture => {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("painted cover rectangle");
                assert!(
                    (painted.width() / painted.height() - source[0] as f32 / source[1] as f32)
                        .abs()
                        < 0.001,
                    "{source:?} -> {painted:?}"
                );
                assert!(painted.width() <= size.x + 0.01 && painted.height() <= size.y + 0.01);
                let mut bounds = egui::Rect::NOTHING;
                for primitive in ctx.tessellate(output.shapes, output.pixels_per_point) {
                    if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                        if mesh.texture_id == texture {
                            for vertex in mesh.vertices {
                                bounds.extend_with(vertex.pos);
                            }
                        }
                    }
                }
                // Rounded image tessellation adds an antialias fringe; it is
                // not part of the image's scale or intrinsic aspect ratio.
                assert!(
                    bounds.is_finite()
                        && painted
                            .expand(1.0 / output.pixels_per_point)
                            .contains_rect(bounds)
                );
                let mut next_missing = egui::Rect::NOTHING;
                let _ = ctx.run(raw, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        assert!(!cache.show(ui, None, Some("fixture:missing"), size, 4.0));
                        ui.allocate_exact_size(size, egui::Sense::hover());
                        next_missing = ui.label("next").rect;
                    });
                });
                assert_eq!(
                    next_cached, next_missing,
                    "loaded and missing covers keep the same slot"
                );
            }
        }
    }
}
