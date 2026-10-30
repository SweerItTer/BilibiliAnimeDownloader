//! Read-only cover thumbnails fetched through core's validated cover path.
use crate::backend::{Bridge, Command};
use std::collections::{HashMap, HashSet};

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
        let Some(id) = self.texture(url) else {
            return false;
        };
        ui.add(egui::Image::new((id, size)).rounding(rounding));
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
}
