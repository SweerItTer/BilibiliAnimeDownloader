//! Shared visual tokens and native painter widgets; no remote assets.
use egui::{pos2, vec2, Align2, Color32, FontId, Frame, Id, Rect, Response, Sense, Stroke, Ui};
use std::cell::Cell;
use std::time::Duration;

pub const OUTER_RADIUS: f32 = 10.0;
pub const INSET: f32 = 8.0;
pub const MOTION: f32 = 0.18;
pub const ON_PRIMARY: Color32 = Color32::WHITE;
pub const ERROR: Color32 = Color32::from_rgb(174, 62, 85);
pub const GREEN: Color32 = Color32::from_rgb(33, 111, 80);

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub dark_mode: bool,
    pub background: Color32,
    pub card: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub primary: Color32,
    pub primary_hover: Color32,
    pub on_primary: Color32,
    pub soft: Color32,
    pub soft_hover: Color32,
    pub border: Color32,
    pub rose: Color32,
    pub error: Color32,
    pub success: Color32,
}

pub const fn light_palette(primary: Color32, primary_hover: Color32) -> Palette {
    Palette {
        dark_mode: false,
        background: Color32::from_rgb(240, 242, 245),
        card: Color32::from_rgb(250, 251, 252),
        ink: Color32::from_rgb(32, 37, 45),
        muted: Color32::from_rgb(89, 99, 113),
        primary,
        primary_hover,
        on_primary: ON_PRIMARY,
        soft: Color32::from_rgb(232, 234, 240),
        soft_hover: Color32::from_rgb(221, 225, 233),
        border: Color32::from_rgb(164, 173, 186),
        rose: Color32::from_rgb(248, 232, 236),
        error: ERROR,
        success: GREEN,
    }
}

pub const fn dark_palette(primary: Color32, primary_hover: Color32) -> Palette {
    Palette {
        dark_mode: true,
        background: Color32::from_rgb(22, 25, 31),
        card: Color32::from_rgb(31, 35, 43),
        ink: Color32::from_rgb(228, 232, 239),
        muted: Color32::from_rgb(162, 173, 190),
        primary,
        primary_hover,
        on_primary: Color32::from_rgb(18, 22, 28),
        soft: Color32::from_rgb(43, 48, 59),
        soft_hover: Color32::from_rgb(54, 60, 72),
        border: Color32::from_rgb(94, 105, 124),
        rose: Color32::from_rgb(62, 36, 44),
        error: Color32::from_rgb(244, 143, 157),
        success: Color32::from_rgb(137, 207, 166),
    }
}

pub const DEFAULT_PALETTE: Palette = light_palette(
    Color32::from_rgb(99, 81, 171),
    Color32::from_rgb(80, 65, 147),
);

thread_local! {
    // A native app paints on its UI thread. Independent headless contexts on
    // other threads must not replace one another's current presentation tokens.
    static PALETTE: Cell<Palette> = const { Cell::new(DEFAULT_PALETTE) };
}

pub fn set_palette(palette: Palette) {
    PALETTE.set(palette);
}

pub fn palette() -> Palette {
    PALETTE.get()
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
pub fn error_color() -> Color32 {
    palette().error
}
pub fn success_color() -> Color32 {
    palette().success
}

pub fn inner_radius(outer: f32, inset: f32) -> f32 {
    (outer - inset).max(0.0)
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
    style.visuals = if p.dark_mode {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
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
    style.visuals.error_fg_color = p.error;
    style.visuals.warn_fg_color = if p.dark_mode {
        Color32::from_rgb(233, 187, 112)
    } else {
        Color32::from_rgb(139, 88, 19)
    };
    style.visuals.widgets.noninteractive.rounding = egui::Rounding::same(6.0);
    style.visuals.widgets.noninteractive.bg_fill = p.card;
    style.visuals.widgets.noninteractive.weak_bg_fill = p.card;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.ink);
    style.visuals.widgets.inactive.rounding = egui::Rounding::same(6.0);
    style.visuals.widgets.inactive.bg_fill = p.background;
    style.visuals.widgets.inactive.weak_bg_fill = p.background;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.ink);
    style.visuals.widgets.hovered.rounding = egui::Rounding::same(6.0);
    style.visuals.widgets.hovered.bg_fill = p.soft;
    style.visuals.widgets.hovered.weak_bg_fill = p.soft;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.primary);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.primary);
    style.visuals.widgets.active.rounding = egui::Rounding::same(6.0);
    style.visuals.widgets.active.bg_fill = p.soft;
    style.visuals.widgets.active.weak_bg_fill = p.soft;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.5_f32, p.primary);
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, p.primary);
    style.visuals.widgets.open = style.visuals.widgets.active;
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

/// Single-line controls share text metrics even when a caller gives the frame
/// extra height. Explicit hint color avoids inheriting the app's ink override.
pub fn singleline_input<'a>(
    ui: &Ui,
    text: &'a mut dyn egui::TextBuffer,
    hint: &str,
    height: f32,
) -> egui::TextEdit<'a> {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let row_height = ui.fonts(|fonts| fonts.row_height(&font));
    let height = height.max(row_height + 4.0);
    // egui 0.29 hit testing starts at the inner rect's top. Put the extra
    // height in symmetric margins so painting and pointer coordinates agree.
    egui::TextEdit::singleline(text)
        .font(font)
        .margin(vec2(4.0, (height - row_height) * 0.5))
        .hint_text_font(egui::TextStyle::Body)
        .hint_text(
            egui::RichText::new(hint)
                .text_style(egui::TextStyle::Body)
                .color(muted_color()),
        )
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

#[derive(Clone, Copy)]
pub enum Icon {
    Download,
    Check,
    User,
    Arrow,
    Back,
    Play,
    Close,
    Refresh,
    Folder,
    Image,
    Palette,
    Crop,
    Minus,
    Window,
    Restore,
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
        Icon::Download => {
            line(&[(0.0, -9.0), (0.0, 3.0)]);
            line(&[(-4.0, -1.0), (0.0, 3.0), (4.0, -1.0)]);
            line(&[(-8.0, 2.0), (-8.0, 8.0), (8.0, 8.0), (8.0, 2.0)]);
        }
        Icon::Check => line(&[(-7.0, 0.0), (-2.0, 5.0), (8.0, -5.0)]),

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

        Icon::Play => line(&[(-5.0, -8.0), (8.0, 0.0), (-5.0, 8.0), (-5.0, -8.0)]),
        Icon::Close => {
            line(&[(-6.0, -6.0), (6.0, 6.0)]);
            line(&[(6.0, -6.0), (-6.0, 6.0)]);
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
        ButtonKind::Primary => (p.primary, p.primary_hover, p.on_primary),
        ButtonKind::Soft => (p.soft, p.soft_hover, p.primary),
        ButtonKind::Quiet => (p.card, p.soft, p.muted),
        ButtonKind::Danger => (p.rose, mix(p.rose, p.error, 0.12), p.error),
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
        // Disabled egui UIs already tint painter colors toward the card.
        // Keep the input opaque: fading alpha again washes out light-mode text.
        p.ink
    };
    let painted = if pressed { rect.shrink(1.0) } else { rect };
    ui.painter().rect(
        painted,
        6.0,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_ui_threads_keep_their_own_palette() {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = [false, true]
            .into_iter()
            .map(|dark| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let expected = crate::prefs::THEMES[2].palette_for(dark);
                    set_palette(expected);
                    barrier.wait();
                    assert!(palette() == expected);
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn primary_button_paints_the_mode_specific_foreground() {
        for dark in [false, true] {
            let ctx = egui::Context::default();
            let expected = crate::prefs::THEMES[1].palette_for(dark);
            set_palette(expected);
            apply_style(&ctx, false);
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 200.0))),
                ..Default::default()
            };
            let output = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    button(ui, "Start", Icon::Download, ButtonKind::Primary, true);
                });
            });
            fn color(shape: &egui::Shape) -> Option<Color32> {
                match shape {
                    egui::Shape::Text(text) if text.galley.job.text == "Start" => {
                        Some(text.fallback_color)
                    }
                    egui::Shape::Vec(children) => children.iter().find_map(color),
                    _ => None,
                }
            }
            let actual = output
                .shapes
                .iter()
                .find_map(|shape| color(&shape.shape))
                .unwrap();
            assert_eq!(actual, expected.on_primary);
        }
    }

    #[test]
    fn nested_corners_share_the_same_center() {
        assert_eq!(inner_radius(OUTER_RADIUS, INSET), 2.0);
        assert_eq!(inner_radius(6.0, 8.0), 0.0);
    }

    fn input_frame(
        ctx: &egui::Context,
        text: &mut String,
        height: f32,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, egui::text_edit::TextEditOutput) {
        let mut edit = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(600.0, 250.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    edit = Some(
                        ui.allocate_ui_with_layout(
                            vec2(240.0, height),
                            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                            |ui| {
                                singleline_input(ui, text, "Search hint", height)
                                    .id_salt("input-under-test")
                                    .show(ui)
                            },
                        )
                        .inner,
                    );
                });
            },
        );
        (output, edit.unwrap())
    }
    #[test]
    fn singleline_text_and_hint_are_centered_with_distinct_normal_hint_color() {
        for height in [30.0, 34.0, 54.0] {
            for value in ["", "Entered value"] {
                let ctx = egui::Context::default();
                apply_style(&ctx, false);
                let mut text = value.to_owned();
                let (output, edit) = input_frame(&ctx, &mut text, height, vec![]);
                let expected = if value.is_empty() {
                    "Search hint"
                } else {
                    value
                };
                fn find(
                    shapes: &[egui::epaint::ClippedShape],
                    text: &str,
                ) -> Option<egui::epaint::TextShape> {
                    fn shape(node: &egui::Shape, text: &str) -> Option<egui::epaint::TextShape> {
                        match node {
                            egui::Shape::Text(t) if t.galley.job.text == text => Some(t.clone()),
                            egui::Shape::Vec(v) => v.iter().find_map(|s| shape(s, text)),
                            _ => None,
                        }
                    }
                    shapes.iter().find_map(|s| shape(&s.shape, text))
                }
                let painted = find(&output.shapes, expected).expect("painted input text");
                let bounds = painted.galley.rect.translate(painted.pos.to_vec2());
                assert!(
                    (bounds.center().y - edit.response.rect.center().y).abs() < 0.6,
                    "{height}: {bounds:?} in {:?}",
                    edit.response.rect
                );
                let format = &painted.galley.job.sections[0].format;
                assert_eq!(format.font_id, egui::TextStyle::Body.resolve(&ctx.style()));
                assert_eq!(
                    format.color,
                    if value.is_empty() {
                        muted_color()
                    } else {
                        ink()
                    }
                );
            }
        }
    }
    #[test]
    fn pointer_clicks_in_both_halves_of_centered_text_insert_at_the_clicked_character() {
        for height in [30.0, 34.0, 54.0] {
            for y in [0.25, 0.5, 0.8] {
                for index in [0, 4, 10] {
                    let ctx = egui::Context::default();
                    apply_style(&ctx, false);
                    let mut text = "abcdefghij".to_owned();
                    let (_, edit) = input_frame(&ctx, &mut text, height, vec![]);
                    let caret = edit
                        .galley
                        .pos_from_ccursor(egui::text::CCursor::new(index));
                    let pos = edit.galley_pos + vec2(caret.left() + 0.1, edit.galley.size().y * y);
                    let _ = input_frame(
                        &ctx,
                        &mut text,
                        height,
                        vec![
                            egui::Event::PointerMoved(pos),
                            egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed: true,
                                modifiers: Default::default(),
                            },
                        ],
                    );
                    let _ = input_frame(
                        &ctx,
                        &mut text,
                        height,
                        vec![egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: Default::default(),
                        }],
                    );
                    let _ =
                        input_frame(&ctx, &mut text, height, vec![egui::Event::Text("|".into())]);
                    let mut expected = "abcdefghij".to_owned();
                    expected.insert(index, '|');
                    assert_eq!(text, expected, "height={height}, y={y}, index={index}");
                }
            }
        }
    }
}
