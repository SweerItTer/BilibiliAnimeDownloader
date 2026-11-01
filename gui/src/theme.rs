//! Shared visual tokens and native painter widgets; no remote assets.
use egui::{pos2, vec2, Align2, Color32, FontId, Frame, Id, Rect, Response, Sense, Stroke, Ui};
use std::sync::RwLock;
use std::time::Duration;

pub const OUTER_RADIUS: f32 = 24.0;
pub const INSET: f32 = 8.0;
pub const MOTION: f32 = 0.18;
pub const ON_PRIMARY: Color32 = Color32::WHITE;
pub const ERROR: Color32 = Color32::from_rgb(174, 62, 85);
pub const GREEN: Color32 = Color32::from_rgb(50, 132, 112);

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub background: Color32,
    pub card: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub primary: Color32,
    pub primary_hover: Color32,
    pub soft: Color32,
    pub soft_hover: Color32,
    pub border: Color32,
    pub rose: Color32,
}

pub const DEFAULT_PALETTE: Palette = Palette {
    background: Color32::from_rgb(245, 245, 250),
    card: Color32::WHITE,
    ink: Color32::from_rgb(36, 36, 56),
    muted: Color32::from_rgb(112, 112, 132),
    primary: Color32::from_rgb(115, 87, 217),
    primary_hover: Color32::from_rgb(97, 68, 195),
    soft: Color32::from_rgb(239, 235, 252),
    soft_hover: Color32::from_rgb(226, 218, 248),
    border: Color32::from_rgb(229, 228, 238),
    rose: Color32::from_rgb(249, 232, 239),
};

static PALETTE: RwLock<Palette> = RwLock::new(DEFAULT_PALETTE);

pub fn set_palette(palette: Palette) {
    if let Ok(mut current) = PALETTE.write() {
        *current = palette;
    }
}

pub fn palette() -> Palette {
    PALETTE.read().map(|p| *p).unwrap_or(DEFAULT_PALETTE)
}

pub fn background() -> Color32 {
    palette().background
}
pub fn card() -> Color32 {
    palette().card
}
pub fn ink() -> Color32 {
    palette().ink
}
pub fn muted_color() -> Color32 {
    palette().muted
}
pub fn primary() -> Color32 {
    palette().primary
}
pub fn soft() -> Color32 {
    palette().soft
}
pub fn border() -> Color32 {
    palette().border
}
pub fn rose() -> Color32 {
    palette().rose
}

pub fn inner_radius(outer: f32, inset: f32) -> f32 {
    (outer - inset).max(0.0)
}

/// Blend `fill` toward transparent by `keep` (1.0 keeps the color opaque).
pub fn alpha(fill: Color32, keep: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        fill.r(),
        fill.g(),
        fill.b(),
        (keep.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

pub fn install(ctx: &egui::Context, fonts: Option<egui::FontDefinitions>) {
    if let Some(fonts) = fonts {
        ctx.set_fonts(fonts);
    }
    apply_style(ctx, false);
}

pub fn apply_style(ctx: &egui::Context, compact: bool) {
    let p = palette();
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::light();
    style.visuals.panel_fill = p.background;
    style.visuals.window_fill = p.card;
    style.visuals.window_rounding = egui::Rounding::same(OUTER_RADIUS);
    style.visuals.window_stroke = Stroke::new(1.0_f32, p.border);
    style.visuals.extreme_bg_color = p.card;
    style.visuals.faint_bg_color = p.background;
    style.visuals.override_text_color = Some(p.ink);
    style.visuals.selection.bg_fill = p.soft;
    style.visuals.selection.stroke = Stroke::new(1.0_f32, p.primary);
    style.visuals.hyperlink_color = p.primary;
    style.visuals.widgets.inactive.rounding = egui::Rounding::same(12.0);
    style.visuals.widgets.inactive.bg_fill = p.background;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.ink);
    style.visuals.widgets.hovered.rounding = egui::Rounding::same(12.0);
    style.visuals.widgets.hovered.bg_fill = p.soft;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.primary);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.primary);
    style.visuals.widgets.active.rounding = egui::Rounding::same(12.0);
    style.visuals.widgets.active.bg_fill = p.soft;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.5_f32, p.primary);
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, p.primary);
    style.spacing.item_spacing = if compact {
        vec2(8.0, 6.0)
    } else {
        vec2(10.0, 10.0)
    };
    style.spacing.button_padding = if compact {
        vec2(12.0, 7.0)
    } else {
        vec2(16.0, 10.0)
    };
    style.spacing.interact_size = if compact {
        vec2(34.0, 32.0)
    } else {
        vec2(40.0, 38.0)
    };
    style
        .text_styles
        .insert(egui::TextStyle::Heading, FontId::proportional(26.0));
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(12.0));
    ctx.set_style(style);
}

/// Loaded by the host before the paint loop. Use installed fonts, never bundle
/// a proprietary system font or fetch one from the network.
pub fn system_fonts() -> Option<egui::FontDefinitions> {
    let mut paths = Vec::new();
    if let Some(windows) = std::env::var_os("WINDIR") {
        let fonts = std::path::PathBuf::from(windows).join("Fonts");
        paths.extend([
            fonts.join("msyh.ttc"),
            fonts.join("simhei.ttf"),
            fonts.join("simsun.ttc"),
        ]);
    }
    paths.extend([
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into(),
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc".into(),
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc".into(),
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc".into(),
    ]);
    for path in paths {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("installed-cjk".into(), egui::FontData::from_owned(bytes));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "installed-cjk".into());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("installed-cjk".into());
            return Some(fonts);
        }
    }
    None
}

pub fn card_frame() -> Frame {
    Frame::none()
        .fill(card())
        .stroke(Stroke::new(1.0_f32, border()))
        .rounding(OUTER_RADIUS)
        .inner_margin(INSET)
}

pub fn inset(fill: Color32) -> Frame {
    Frame::none()
        .fill(fill)
        .rounding(inner_radius(OUTER_RADIUS, INSET))
        .inner_margin(16.0)
}

pub fn muted(ui: &mut Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text).color(muted_color()));
}

/// Secondary copy that supports a control, one step smaller than body text.
pub fn hint(ui: &mut Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text).size(12.0).color(muted_color()));
}

pub fn section(ui: &mut Ui, title: &str, description: &str) {
    ui.label(egui::RichText::new(title).size(17.0).strong());
    ui.add_space(3.0);
    hint(ui, description);
    ui.add_space(10.0);
}

pub fn hint_indented(ui: &mut Ui, id: &str, text: impl Into<String>) {
    ui.indent(id, |ui| hint(ui, text));
}

/// Hollow in-app brand mark. The filled raster icon is reserved for the OS shell.
pub fn brand_mark(painter: &egui::Painter, rect: Rect, accent: Color32) {
    let rounding = rect.width() * 0.32;
    painter.rect_filled(rect, rounding, alpha(accent, 0.08));
    painter.rect_stroke(rect, rounding, Stroke::new(1.8_f32, accent));
    icon(
        painter,
        rect.shrink(rect.width() * 0.27),
        Icon::Download,
        accent,
    );
    let dot = rect.width() * 0.075;
    painter.circle_filled(
        pos2(rect.right() - dot * 1.4, rect.top() + dot * 1.4),
        dot,
        Color32::from_rgb(255, 126, 168),
    );
}

#[derive(Clone, Copy)]
pub enum Icon {
    Search,
    Download,
    Check,
    Settings,
    User,
    Arrow,
    Back,
    Pause,
    Play,
    Close,
    Copy,
    Refresh,
    Folder,
    Image,
    Palette,
    Crop,
    Minus,
    Window,
    Restore,
    Info,
}

pub fn icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) / 24.0;
    let p = |x: f32, y: f32| c + vec2(x * s, y * s);
    let stroke = Stroke::new(1.7 * s, color);
    let line = |points: &[(f32, f32)]| {
        painter.add(egui::Shape::line(
            points.iter().map(|&(x, y)| p(x, y)).collect(),
            stroke,
        ));
    };
    match icon {
        Icon::Search => {
            painter.circle_stroke(p(-2.0, -2.0), 6.0 * s, stroke);
            line(&[(2.5, 2.5), (8.0, 8.0)]);
        }
        Icon::Download => {
            line(&[(0.0, -9.0), (0.0, 3.0)]);
            line(&[(-4.0, -1.0), (0.0, 3.0), (4.0, -1.0)]);
            line(&[(-8.0, 2.0), (-8.0, 8.0), (8.0, 8.0), (8.0, 2.0)]);
        }
        Icon::Check => line(&[(-7.0, 0.0), (-2.0, 5.0), (8.0, -5.0)]),
        Icon::Settings => {
            for (y, x) in [(-6.0, -3.0), (0.0, 4.0), (6.0, -1.0)] {
                line(&[(-9.0, y), (9.0, y)]);
                painter.circle_filled(p(x, y), 2.5 * s, card());
                painter.circle_stroke(p(x, y), 2.5 * s, stroke);
            }
        }
        Icon::User => {
            painter.circle_stroke(p(0.0, -5.0), 4.0 * s, stroke);
            line(&[
                (-8.0, 9.0),
                (-7.0, 4.0),
                (-3.0, 2.0),
                (3.0, 2.0),
                (7.0, 4.0),
                (8.0, 9.0),
            ]);
        }
        Icon::Arrow => {
            line(&[(-8.0, 0.0), (8.0, 0.0)]);
            line(&[(2.0, -6.0), (8.0, 0.0), (2.0, 6.0)]);
        }
        Icon::Back => {
            line(&[(8.0, 0.0), (-8.0, 0.0)]);
            line(&[(-2.0, -6.0), (-8.0, 0.0), (-2.0, 6.0)]);
        }
        Icon::Pause => {
            line(&[(-4.0, -7.0), (-4.0, 7.0)]);
            line(&[(4.0, -7.0), (4.0, 7.0)]);
        }
        Icon::Play => line(&[(-5.0, -8.0), (8.0, 0.0), (-5.0, 8.0), (-5.0, -8.0)]),
        Icon::Close => {
            line(&[(-6.0, -6.0), (6.0, 6.0)]);
            line(&[(6.0, -6.0), (-6.0, 6.0)]);
        }
        Icon::Copy => {
            painter.rect_stroke(
                Rect::from_min_max(p(-4.0, -4.0), p(8.0, 9.0)),
                2.0 * s,
                stroke,
            );
            line(&[
                (-7.0, 5.0),
                (-9.0, 5.0),
                (-9.0, -9.0),
                (3.0, -9.0),
                (3.0, -7.0),
            ]);
        }
        Icon::Refresh => {
            line(&[
                (8.0, -2.0),
                (6.0, -6.0),
                (0.0, -8.0),
                (-6.0, -5.0),
                (-8.0, 1.0),
                (-5.0, 7.0),
                (2.0, 8.0),
                (7.0, 5.0),
            ]);
            line(&[(3.0, -2.0), (8.0, -2.0), (8.0, -8.0)]);
        }
        Icon::Folder => line(&[
            (-9.0, 7.0),
            (-9.0, -7.0),
            (-2.0, -7.0),
            (0.0, -4.0),
            (9.0, -4.0),
            (9.0, 7.0),
            (-9.0, 7.0),
        ]),
        Icon::Image => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(18.0 * s, 15.0 * s)),
                3.0 * s,
                stroke,
            );
            painter.circle_stroke(p(3.0, -3.0), 1.8 * s, stroke);
            line(&[(-7.0, 5.0), (-2.0, 0.0), (1.0, 3.0), (4.0, 0.0), (8.0, 5.0)]);
        }
        Icon::Palette => {
            painter.circle_stroke(c, 8.0 * s, stroke);
            for (x, y) in [(-4.0, -3.0), (2.0, -5.0), (4.0, 2.0), (-3.0, 4.0)] {
                painter.circle_filled(p(x, y), 1.6 * s, color);
            }
        }
        Icon::Crop => {
            line(&[(-7.0, 6.0), (-7.0, -7.0), (6.0, -7.0)]);
            line(&[(7.0, -6.0), (7.0, 7.0), (-6.0, 7.0)]);
        }
        Icon::Minus => line(&[(-7.0, 0.0), (7.0, 0.0)]),
        Icon::Window => {
            painter.rect_stroke(
                Rect::from_center_size(c, vec2(14.0, 13.0) * s),
                2.0 * s,
                stroke,
            );
        }
        Icon::Restore => {
            painter.rect_stroke(
                Rect::from_center_size(p(-1.5, 1.5), vec2(12.0, 11.0) * s),
                2.0 * s,
                stroke,
            );
            painter.rect_stroke(
                Rect::from_center_size(p(1.5, -1.5), vec2(12.0, 11.0) * s),
                2.0 * s,
                stroke,
            );
        }
        Icon::Info => {
            painter.circle_stroke(c, 8.5 * s, stroke);
            line(&[(0.0, -1.5), (0.0, 5.0)]);
            painter.circle_filled(p(0.0, -5.0), 1.1 * s, color);
        }
    }
}

#[derive(Clone, Copy)]
pub enum ButtonKind {
    Primary,
    Soft,
    Quiet,
    Danger,
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

pub fn button_width_for(ui: &Ui, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(14.0), ink())
        .size()
        .x
        + 62.0
}

pub fn button(ui: &mut Ui, text: &str, symbol: Icon, kind: ButtonKind, reduced: bool) -> Response {
    let width = button_width_for(ui, text);
    button_width(ui, text, symbol, kind, width, reduced)
}

pub fn button_width(
    ui: &mut Ui,
    text: &str,
    symbol: Icon,
    kind: ButtonKind,
    width: f32,
    reduced: bool,
) -> Response {
    let height = if ui.spacing().item_spacing.y < 8.0 {
        36.0
    } else {
        42.0
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), text));
    let highlighted = response.hovered() || response.has_focus();
    let t = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        highlighted,
        if reduced { 0.0 } else { MOTION },
    );
    let p = palette();
    let (base, hover, foreground) = match kind {
        ButtonKind::Primary => (p.primary, p.primary_hover, ON_PRIMARY),
        ButtonKind::Soft => (p.soft, p.soft_hover, p.primary),
        ButtonKind::Quiet => (p.card, p.soft, p.muted),
        ButtonKind::Danger => (p.rose, mix(p.rose, ERROR, 0.12), ERROR),
    };
    let pressed = response.is_pointer_button_down_on();
    let fill = if !ui.is_enabled() {
        p.background
    } else {
        mix(base, hover, if pressed { 1.0 } else { t })
    };
    let foreground = if ui.is_enabled() {
        foreground
    } else {
        p.muted.gamma_multiply(0.55)
    };
    let painted = if pressed { rect.shrink(1.0) } else { rect };
    ui.painter().rect(
        painted,
        12.0,
        fill,
        if response.has_focus() {
            Stroke::new(2.0_f32, p.primary)
        } else {
            Stroke::NONE
        },
    );
    icon(
        ui.painter(),
        Rect::from_center_size(
            pos2(painted.left() + 24.0, painted.center().y),
            vec2(20.0, 20.0),
        ),
        symbol,
        foreground,
    );
    ui.painter().text(
        pos2(painted.left() + 43.0, painted.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(14.0),
        foreground,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn loading(ui: &mut Ui, reduced: bool, label: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        let angle = if reduced {
            0.0
        } else {
            ui.input(|i| i.time) as f32 * 4.0
        };
        let points = (0..28)
            .map(|i| {
                let a = angle + i as f32 / 28.0 * std::f32::consts::TAU * 0.75;
                rect.center() + vec2(a.cos(), a.sin()) * 7.0
            })
            .collect();
        ui.painter()
            .add(egui::Shape::line(points, Stroke::new(2.0_f32, primary())));
        muted(ui, label);
    });
    if !reduced {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
}

pub fn progress(ui: &mut Ui, id: Id, value: Option<f32>, active: bool, reduced: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    ui.painter().rect_filled(rect, 3.0, border());
    if let Some(value) = value {
        let value = ui.ctx().animate_value_with_time(
            id,
            value.clamp(0.0, 1.0),
            if reduced { 0.0 } else { MOTION },
        );
        if value > 0.0 {
            ui.painter().rect_filled(
                Rect::from_min_size(rect.min, vec2(rect.width() * value, rect.height())),
                3.0,
                primary(),
            );
        }
    } else if active {
        let offset = if reduced {
            0.35
        } else {
            ((ui.input(|i| i.time) as f32 * 1.5).sin() + 1.0) * 0.35
        };
        let track = Rect::from_min_size(
            rect.min + vec2(rect.width() * offset, 0.0),
            vec2(rect.width() * 0.3, rect.height()),
        );
        ui.painter().rect_filled(track, 3.0, primary());
        if !reduced {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }
}

pub fn empty_art(ui: &mut Ui) {
    let p = palette();
    let (rect, _) = ui.allocate_exact_size(vec2(240.0, 178.0), Sense::hover());
    let painter = ui.painter();
    let c = rect.center();
    painter.circle_filled(c, 76.0, p.soft);
    painter.circle_filled(c + vec2(77.0, -40.0), 16.0, p.rose);
    painter.circle_filled(c + vec2(-84.0, 26.0), 7.0, mix(p.soft, p.primary, 0.25));
    let sheet = Rect::from_center_size(c + vec2(0.0, -18.0), vec2(78.0, 94.0));
    painter.rect(
        sheet.translate(vec2(10.0, -6.0)),
        16.0,
        mix(p.soft, p.primary, 0.18),
        Stroke::NONE,
    );
    painter.rect(sheet, 16.0, p.card, Stroke::new(1.0_f32, p.border));
    icon(
        painter,
        Rect::from_center_size(sheet.center() - vec2(0.0, 4.0), vec2(36.0, 36.0)),
        Icon::Download,
        p.primary,
    );
    let tray = Rect::from_center_size(c + vec2(0.0, 39.0), vec2(132.0, 53.0));
    painter.rect_filled(tray, 20.0, p.primary);
    painter.rect_filled(
        Rect::from_center_size(tray.center() - vec2(0.0, 8.0), vec2(116.0, 21.0)),
        12.0,
        mix(p.primary, Color32::WHITE, 0.32),
    );
    painter.line_segment(
        [
            tray.center() + vec2(-13.0, 10.0),
            tray.center() + vec2(13.0, 10.0),
        ],
        Stroke::new(3.0_f32, ON_PRIMARY),
    );
    for offset in [vec2(-80.0, -52.0), vec2(84.0, 50.0)] {
        painter.line_segment(
            [c + offset - vec2(4.0, 0.0), c + offset + vec2(4.0, 0.0)],
            Stroke::new(1.5_f32, p.primary),
        );
        painter.line_segment(
            [c + offset - vec2(0.0, 4.0), c + offset + vec2(0.0, 4.0)],
            Stroke::new(1.5_f32, p.primary),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_corners_share_the_same_center() {
        assert_eq!(inner_radius(OUTER_RADIUS, INSET), 16.0);
        assert_eq!(inner_radius(6.0, 8.0), 0.0);
    }

    #[test]
    fn translucent_overlay_keeps_requested_alpha() {
        let color = alpha(Color32::from_rgb(10, 20, 30), 0.5);
        assert_eq!(color.a(), 128);
        assert_eq!(alpha(ON_PRIMARY, 2.0).a(), 255);
    }
}
