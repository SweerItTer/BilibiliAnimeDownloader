//! Shell-owned presentation preferences. Never stores credentials or QR data.
use crate::theme::{self, Palette};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub struct ThemePreset {
    pub id: &'static str,
    pub name: &'static str,
    pub palette: Palette,
}

const fn rgb(v: (u8, u8, u8)) -> egui::Color32 {
    egui::Color32::from_rgb(v.0, v.1, v.2)
}

const fn p(colors: [(u8, u8, u8); 9]) -> Palette {
    Palette {
        background: rgb(colors[0]),
        card: egui::Color32::WHITE,
        ink: rgb(colors[1]),
        muted: rgb(colors[2]),
        primary: rgb(colors[3]),
        primary_hover: rgb(colors[4]),
        soft: rgb(colors[5]),
        soft_hover: rgb(colors[6]),
        border: rgb(colors[7]),
        rose: rgb(colors[8]),
    }
}

pub const THEMES: [ThemePreset; 5] = [
    ThemePreset {
        id: "violet",
        name: "应援紫",
        palette: theme::DEFAULT_PALETTE,
    },
    ThemePreset {
        id: "2233-pink",
        name: "2233 粉",
        palette: p([
            (255, 247, 249),
            (58, 48, 54),
            (139, 124, 132),
            (251, 114, 153),
            (226, 92, 130),
            (255, 233, 240),
            (255, 214, 228),
            (244, 222, 228),
            (252, 233, 240),
        ]),
    },
    ThemePreset {
        id: "tv-blue",
        name: "小电视蓝",
        palette: p([
            (244, 250, 253),
            (34, 52, 62),
            (108, 130, 142),
            (0, 161, 214),
            (0, 143, 192),
            (225, 246, 252),
            (201, 238, 249),
            (216, 235, 242),
            (253, 235, 239),
        ]),
    },
    ThemePreset {
        id: "matcha",
        name: "抹茶新番",
        palette: p([
            (245, 250, 246),
            (40, 56, 46),
            (112, 130, 118),
            (76, 154, 107),
            (63, 133, 90),
            (228, 243, 233),
            (210, 235, 220),
            (220, 235, 225),
            (251, 239, 240),
        ]),
    },
    ThemePreset {
        id: "sunset",
        name: "落日放送",
        palette: p([
            (252, 247, 243),
            (60, 48, 42),
            (140, 120, 108),
            (217, 122, 74),
            (193, 105, 61),
            (252, 235, 225),
            (249, 220, 203),
            (240, 225, 213),
            (251, 234, 234),
        ]),
    },
];

pub fn theme_index(id: &str) -> usize {
    THEMES.iter().position(|t| t.id == id).unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum BackgroundScope {
    #[serde(rename = "full")]
    Full,
    #[serde(rename = "without-sidebar")]
    WithoutSidebar,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum StartPage {
    Discover,
    Queue,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum UpdateChannel {
    Stable,
    Beta,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuiPrefs {
    pub theme: String,
    pub background: Option<PathBuf>,
    /// Cropped source rectangle in normalized image coordinates: x, y, w, h.
    pub crop: [f32; 4],
    pub background_scope: BackgroundScope,
    /// How strongly the image shows through, 0..100.
    pub opacity: f32,
    /// Background blur strength, 0..100.
    pub blur: f32,
    pub reduced_motion: bool,
    pub compact: bool,
    pub ui_scale: f32,
    pub start_page: StartPage,
    pub confirm_close: bool,
    /// Opt-in marker only: the credential itself lives in the OS keyring.
    pub remember_login: bool,
    pub output: Option<PathBuf>,
    pub auto_check: bool,
    pub update_channel: UpdateChannel,
    /// Unix seconds of the last successful or failed check. No credentials.
    pub last_check: u64,
}

impl Default for GuiPrefs {
    fn default() -> Self {
        Self {
            theme: THEMES[0].id.into(),
            background: None,
            crop: [0.0, 0.0, 1.0, 1.0],
            background_scope: BackgroundScope::Full,
            opacity: 72.0,
            blur: 0.0,
            reduced_motion: false,
            compact: false,
            ui_scale: 1.0,
            start_page: StartPage::Discover,
            confirm_close: true,
            remember_login: false,
            output: None,
            auto_check: false,
            update_channel: UpdateChannel::Stable,
            last_check: 0,
        }
    }
}

impl GuiPrefs {
    pub fn sanitized(mut self) -> Self {
        if THEMES.iter().all(|t| t.id != self.theme) {
            self.theme = THEMES[0].id.into();
        }
        let [x, y, w, h] = self.crop;
        self.crop = [
            x.clamp(0.0, 0.99),
            y.clamp(0.0, 0.99),
            w.clamp(0.02, 1.0),
            h.clamp(0.02, 1.0),
        ];
        self.crop[2] = self.crop[2].min(1.0 - self.crop[0]).max(0.02);
        self.crop[3] = self.crop[3].min(1.0 - self.crop[1]).max(0.02);
        self.opacity = self.opacity.clamp(0.0, 100.0);
        self.blur = self.blur.clamp(0.0, 100.0);
        self.ui_scale = self.ui_scale.clamp(0.8, 1.3);
        self
    }

    fn file() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "bili")
            .map(|dirs| dirs.config_dir().join("gui-preferences.json"))
    }

    pub fn load() -> Self {
        Self::file()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .map(GuiPrefs::sanitized)
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = Self::file() else {
            return;
        };
        let Some(parent) = path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let Ok(bytes) = serde_json::to_vec_pretty(self) else {
            return;
        };
        let pending = path.with_extension("json.tmp");
        if std::fs::write(&pending, bytes).is_ok() {
            let _ = std::fs::rename(pending, path);
        }
    }

    pub fn theme_index(&self) -> usize {
        theme_index(&self.theme)
    }

    pub fn apply(&self, ctx: &egui::Context) {
        theme::set_palette(THEMES[self.theme_index()].palette);
        theme::apply_style(ctx, self.compact);
        ctx.set_zoom_factor(self.ui_scale);
    }

    pub fn set_theme(&mut self, index: usize) {
        self.theme = THEMES[index.min(THEMES.len() - 1)].id.into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_crop_and_scale() {
        let prefs = GuiPrefs {
            crop: [0.9, -1.0, 0.5, 9.0],
            opacity: 500.0,
            ui_scale: 3.0,
            theme: "missing".into(),
            ..GuiPrefs::default()
        }
        .sanitized();
        assert_eq!(prefs.theme, THEMES[0].id);
        assert!((prefs.crop[0] - 0.9).abs() < f32::EPSILON);
        assert!((prefs.crop[2] - 0.1).abs() < 1e-6);
        assert_eq!(prefs.opacity, 100.0);
        assert_eq!(prefs.ui_scale, 1.3);
    }

    #[test]
    fn prefs_round_trip_without_credentials() {
        let prefs = GuiPrefs {
            theme: THEMES[2].id.into(),
            background: Some(PathBuf::from("C:/pictures/wall.jpg")),
            ..GuiPrefs::default()
        };
        let json = serde_json::to_string(&prefs).unwrap();
        let back: GuiPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(back, prefs);
        assert!(!json.contains("cookie"));
        assert!(!json.contains("token"));
    }
}
