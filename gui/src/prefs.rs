//! Shell-owned presentation preferences. Never stores credentials or QR data.
use crate::theme::{self, Palette};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub struct ThemePreset {
    pub id: &'static str,
    pub name: &'static str,
    pub palette: Palette,
    dark_primary: egui::Color32,
    dark_primary_hover: egui::Color32,
}

impl ThemePreset {
    pub fn palette_for(&self, dark: bool) -> Palette {
        if dark {
            theme::dark_palette(self.dark_primary, self.dark_primary_hover)
        } else {
            self.palette
        }
    }
}

const fn rgb(v: (u8, u8, u8)) -> egui::Color32 {
    egui::Color32::from_rgb(v.0, v.1, v.2)
}

pub const THEMES: [ThemePreset; 5] = [
    ThemePreset {
        id: "violet",
        name: "紫色",
        palette: theme::DEFAULT_PALETTE,
        dark_primary: rgb((176, 160, 234)),
        dark_primary_hover: rgb((195, 182, 248)),
    },
    ThemePreset {
        id: "2233-pink",
        name: "粉色",
        palette: theme::light_palette(rgb((166, 58, 96)), rgb((144, 46, 80))),
        dark_primary: rgb((230, 151, 179)),
        dark_primary_hover: rgb((244, 173, 198)),
    },
    ThemePreset {
        id: "tv-blue",
        name: "蓝色",
        palette: theme::light_palette(rgb((24, 103, 151)), rgb((20, 86, 128))),
        dark_primary: rgb((113, 190, 230)),
        dark_primary_hover: rgb((145, 208, 240)),
    },
    ThemePreset {
        id: "matcha",
        name: "绿色",
        palette: theme::light_palette(rgb((32, 111, 75)), rgb((27, 93, 61))),
        dark_primary: rgb((116, 196, 151)),
        dark_primary_hover: rgb((145, 214, 175)),
    },
    ThemePreset {
        id: "sunset",
        name: "橙色",
        palette: theme::light_palette(rgb((151, 75, 32)), rgb((132, 62, 24))),
        dark_primary: rgb((228, 173, 124)),
        dark_primary_hover: rgb((243, 194, 151)),
    },
];

pub fn theme_index(id: &str) -> usize {
    THEMES.iter().position(|t| t.id == id).unwrap_or(0)
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppearanceMode {
    #[default]
    System,
    Light,
    Dark,
}

impl AppearanceMode {
    pub fn resolved_dark(self, system_theme: Option<egui::Theme>) -> bool {
        match self {
            Self::System => system_theme.unwrap_or(egui::Theme::Dark) == egui::Theme::Dark,
            Self::Light => false,
            Self::Dark => true,
        }
    }

    fn preference(self) -> egui::ThemePreference {
        match self {
            Self::System => egui::ThemePreference::System,
            Self::Light => egui::ThemePreference::Light,
            Self::Dark => egui::ThemePreference::Dark,
        }
    }
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

pub const MAX_DOWNLOAD_RATE_MIB: f64 = 10240.0;
pub fn sanitize_download_rate(rate: f64) -> f64 {
    if rate.is_finite() {
        rate.clamp(0.0, MAX_DOWNLOAD_RATE_MIB)
    } else {
        0.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuiPrefs {
    pub theme: String,
    /// Light/dark appearance is independent from the accent preset.
    pub appearance: AppearanceMode,
    pub background: Option<PathBuf>,
    /// Cropped source rectangle in normalized image coordinates: x, y, w, h.
    pub crop: [f32; 4],
    pub background_scope: BackgroundScope,
    /// How strongly the image shows through, 0..100.
    pub opacity: f32,
    /// Explicit neutral image brightness, separate from its opacity.
    pub background_brightness: f32,
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
    /// Initial shared batch rate in MiB/s for new drafts; zero is unlimited.
    pub default_download_rate_mib: f64,
    /// Shared protocol preferences; this file never contains account credentials.
    pub update: bili_core::update::UpdateSettings,
    /// Unix seconds of the last successful or failed check. No credentials.
    pub last_check: u64,
}

impl Default for GuiPrefs {
    fn default() -> Self {
        Self {
            theme: THEMES[0].id.into(),
            appearance: AppearanceMode::System,
            background: None,
            crop: [0.0, 0.0, 1.0, 1.0],
            background_scope: BackgroundScope::Full,
            opacity: 72.0,
            background_brightness: 100.0,
            blur: 0.0,
            reduced_motion: false,
            compact: false,
            ui_scale: 1.0,
            start_page: StartPage::Discover,
            confirm_close: true,
            remember_login: false,
            output: None,
            default_download_rate_mib: 0.0,
            update: bili_core::update::UpdateSettings::default(),
            last_check: 0,
        }
    }
}

impl GuiPrefs {
    pub fn update_channel(&self) -> UpdateChannel {
        if self.update.allow_pre_release {
            UpdateChannel::Beta
        } else {
            UpdateChannel::Stable
        }
    }

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
        self.background_brightness = if self.background_brightness.is_finite() {
            self.background_brightness.clamp(20.0, 100.0)
        } else {
            100.0
        };
        self.blur = self.blur.clamp(0.0, 100.0);
        self.ui_scale = self.ui_scale.clamp(0.8, 1.3);
        self.default_download_rate_mib = sanitize_download_rate(self.default_download_rate_mib);
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

    pub fn resolved_dark(&self, ctx: &egui::Context) -> bool {
        self.appearance.resolved_dark(ctx.system_theme())
    }

    pub fn palette_for(&self, dark: bool) -> Palette {
        THEMES[self.theme_index()].palette_for(dark)
    }

    pub fn apply(&self, ctx: &egui::Context) {
        ctx.options_mut(|options| options.fallback_theme = egui::Theme::Dark);
        ctx.set_theme(self.appearance.preference());
        theme::set_palette(self.palette_for(self.resolved_dark(ctx)));
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
    fn appearance_and_brightness_persist_without_changing_the_accent_or_image_opacity() {
        let older: GuiPrefs = serde_json::from_str(r#"{"theme":"tv-blue","opacity":83}"#).unwrap();
        assert_eq!(older.appearance, AppearanceMode::System);
        assert_eq!(older.background_brightness, 100.0);
        assert_eq!(older.opacity, 83.0);
        for mode in [
            AppearanceMode::System,
            AppearanceMode::Light,
            AppearanceMode::Dark,
        ] {
            let prefs = GuiPrefs {
                appearance: mode,
                background_brightness: 45.0,
                ..older.clone()
            };
            let restored: GuiPrefs =
                serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
            assert_eq!(restored, prefs);
            assert_eq!(restored.theme, "tv-blue");
            assert_eq!(restored.opacity, 83.0);
        }
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                GuiPrefs {
                    background_brightness: invalid,
                    ..older.clone()
                }
                .sanitized()
                .background_brightness,
                100.0
            );
        }
        for (value, expected) in [(-10.0, 20.0), (15.0, 20.0), (55.0, 55.0), (150.0, 100.0)] {
            assert_eq!(
                GuiPrefs {
                    background_brightness: value,
                    ..older.clone()
                }
                .sanitized()
                .background_brightness,
                expected
            );
        }
    }

    #[test]
    fn system_appearance_tracks_input_while_manual_modes_keep_their_actual_visuals() {
        for mode in [
            AppearanceMode::System,
            AppearanceMode::Light,
            AppearanceMode::Dark,
        ] {
            let ctx = egui::Context::default();
            let prefs = GuiPrefs {
                appearance: mode,
                ..Default::default()
            };
            for system_theme in [None, Some(egui::Theme::Light), Some(egui::Theme::Dark)] {
                let dark = mode.resolved_dark(system_theme);
                let mut actual = None;
                let _ = ctx.run(
                    egui::RawInput {
                        system_theme,
                        ..Default::default()
                    },
                    |ctx| {
                        prefs.apply(ctx);
                        let style = ctx.style();
                        actual = Some((prefs.resolved_dark(ctx), style.visuals.dark_mode));
                        assert_eq!(style.visuals.panel_fill, prefs.palette_for(dark).background);
                        assert_eq!(style.visuals.window_fill, prefs.palette_for(dark).card);
                        assert_eq!(
                            style.visuals.override_text_color,
                            Some(prefs.palette_for(dark).ink)
                        );
                        assert_eq!(style.visuals.error_fg_color, prefs.palette_for(dark).error);
                    },
                );
                assert_eq!(actual, Some((dark, dark)));
            }
        }
    }

    fn luminance(color: egui::Color32) -> f32 {
        let linear = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        linear(color.r()) * 0.2126 + linear(color.g()) * 0.7152 + linear(color.b()) * 0.0722
    }

    fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
        let a = luminance(a);
        let b = luminance(b);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn both_modes_keep_legible_text_buttons_and_statuses_for_every_accent() {
        for dark in [false, true] {
            let neutral = THEMES[0].palette_for(dark);
            for preset in &THEMES {
                let palette = preset.palette_for(dark);
                assert_eq!(palette.background, neutral.background);
                assert_eq!(palette.card, neutral.card);
                assert_eq!(palette.dark_mode, dark);
                for foreground in [palette.ink, palette.muted] {
                    for background in [
                        palette.background,
                        palette.card,
                        palette.soft,
                        palette.soft_hover,
                    ] {
                        assert!(
                            contrast(foreground, background) >= 4.5,
                            "{} dark={dark} text contrast {}",
                            preset.id,
                            contrast(foreground, background)
                        );
                    }
                }
                for background in [palette.primary, palette.primary_hover] {
                    assert!(
                        contrast(palette.on_primary, background) >= 4.5,
                        "{} dark={dark} button contrast {}",
                        preset.id,
                        contrast(palette.on_primary, background)
                    );
                }
                for foreground in [palette.primary, palette.error, palette.success] {
                    assert!(
                        contrast(foreground, palette.card) >= 4.5,
                        "{} dark={dark} status contrast {}",
                        preset.id,
                        contrast(foreground, palette.card)
                    );
                }
                assert!(contrast(palette.error, palette.rose) >= 4.5);
                assert!(contrast(palette.primary, palette.soft_hover) >= 4.5);
                assert!(if dark {
                    luminance(palette.background) < 0.02
                } else {
                    luminance(palette.background) < 0.92
                });
            }
        }
    }

    #[test]
    fn download_rate_default_is_persistent_bounded_and_backward_compatible() {
        let older: GuiPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(older.default_download_rate_mib, 0.0);
        let prefs = GuiPrefs {
            default_download_rate_mib: 10.0,
            ..Default::default()
        };
        let restored: GuiPrefs =
            serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
        assert_eq!(restored.default_download_rate_mib, 10.0);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -10.0] {
            assert_eq!(
                GuiPrefs {
                    default_download_rate_mib: value,
                    ..Default::default()
                }
                .sanitized()
                .default_download_rate_mib,
                0.0
            );
        }
        assert_eq!(
            GuiPrefs {
                default_download_rate_mib: MAX_DOWNLOAD_RATE_MIB * 2.0,
                ..Default::default()
            }
            .sanitized()
            .default_download_rate_mib,
            MAX_DOWNLOAD_RATE_MIB
        );
    }

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

    #[test]
    fn update_settings_use_shared_keys_and_defaults() {
        let prefs: GuiPrefs = serde_json::from_str("{}").unwrap();
        assert!(prefs.update.check_updates);
        assert!(!prefs.update.allow_pre_release);
        assert!(!prefs.update.auto_update);
        let opted_in: GuiPrefs =
            serde_json::from_str(r#"{"update":{"allow_pre_release":true,"auto_update":true}}"#)
                .unwrap();
        assert_eq!(opted_in.update_channel(), UpdateChannel::Beta);
        assert!(opted_in.update.check_updates);
        assert!(opted_in.update.auto_update);
        let value = serde_json::to_value(opted_in).unwrap();
        assert_eq!(value["update"]["allow_pre_release"], true);
        assert!(value.get("auto_check").is_none());
    }
}
