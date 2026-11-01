//! Frameless window chrome: draggable title bar, window controls, resize edges,
//! and rounded window corners that match QQ-style borders.
use crate::theme;
use egui::{Color32, CursorIcon, Frame, Layout, RichText, Sense, Ui, ViewportCommand};

pub const WINDOW_TITLE: &str = "Bili · 喜欢的，留在身边";
const BAR_HEIGHT: f32 = 46.0;
const EDGE: f32 = 6.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Minimize,
    ToggleMaximize,
    Close,
}

pub fn title_bar(ctx: &egui::Context, fill: Color32, maximized: bool) -> Option<Action> {
    let mut action = None;
    egui::TopBottomPanel::top("window-titlebar")
        .exact_height(BAR_HEIGHT)
        .frame(Frame::none().fill(fill).inner_margin(egui::Margin {
            left: 14.0,
            right: 8.0,
            top: 6.0,
            bottom: 6.0,
        }))
        .show(ctx, |ui| {
            let drag = ui.interact(
                ui.max_rect(),
                ui.id().with("titlebar-drag"),
                Sense::click_and_drag(),
            );
            if drag.drag_started() && !maximized {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            }
            drag.on_hover_cursor(CursorIcon::Default);

            ui.horizontal_centered(|ui| {
                let (logo, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::hover());
                theme::brand_mark(ui.painter(), logo, theme::primary());
                ui.add_space(2.0);
                ui.label(
                    RichText::new("Bili")
                        .size(16.0)
                        .strong()
                        .color(theme::ink()),
                );
                ui.label(
                    RichText::new("喜欢的，留在身边")
                        .size(11.5)
                        .color(theme::muted_color()),
                );
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if window_button(ui, theme::Icon::Close, true) {
                        action = Some(Action::Close);
                    }
                    if window_button(
                        ui,
                        if maximized {
                            theme::Icon::Restore
                        } else {
                            theme::Icon::Window
                        },
                        false,
                    ) {
                        action = Some(Action::ToggleMaximize);
                    }
                    if window_button(ui, theme::Icon::Minus, false) {
                        action = Some(Action::Minimize);
                    }
                });
            });
        });
    action
}

fn window_button(ui: &mut Ui, icon: theme::Icon, danger: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(46.0, 32.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        response.hovered(),
        theme::MOTION,
    );
    if response.hovered() || t > 0.0 {
        let fill = if danger {
            Color32::from_rgb(232, 90, 110)
        } else {
            theme::soft()
        };
        ui.painter().rect_filled(
            rect,
            10.0,
            Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), (t * 235.0) as u8),
        );
    }
    let color = if danger && response.hovered() {
        Color32::WHITE
    } else if response.hovered() {
        theme::primary()
    } else {
        theme::muted_color()
    };
    theme::icon(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0)),
        icon,
        color,
    );
    response.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

/// Starts a native resize when the pointer presses near an undecorated edge.
pub fn resize_edges(ctx: &egui::Context, maximized: bool) {
    if maximized {
        return;
    }
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let screen = ctx.screen_rect();
    let left = pos.x - screen.left() <= EDGE;
    let right = screen.right() - pos.x <= EDGE;
    let top = pos.y - screen.top() <= EDGE;
    let bottom = screen.bottom() - pos.y <= EDGE;
    let direction = match (top, bottom, left, right) {
        (true, _, true, _) => Some(egui::viewport::ResizeDirection::NorthWest),
        (true, _, _, true) => Some(egui::viewport::ResizeDirection::NorthEast),
        (_, true, true, _) => Some(egui::viewport::ResizeDirection::SouthWest),
        (_, true, _, true) => Some(egui::viewport::ResizeDirection::SouthEast),
        (true, _, _, _) => Some(egui::viewport::ResizeDirection::North),
        (_, true, _, _) => Some(egui::viewport::ResizeDirection::South),
        (_, _, true, _) => Some(egui::viewport::ResizeDirection::West),
        (_, _, _, true) => Some(egui::viewport::ResizeDirection::East),
        _ => None,
    };
    let Some(direction) = direction else {
        return;
    };
    ctx.set_cursor_icon(match direction {
        egui::viewport::ResizeDirection::North | egui::viewport::ResizeDirection::South => {
            CursorIcon::ResizeVertical
        }
        egui::viewport::ResizeDirection::East | egui::viewport::ResizeDirection::West => {
            CursorIcon::ResizeHorizontal
        }
        egui::viewport::ResizeDirection::NorthWest | egui::viewport::ResizeDirection::SouthEast => {
            CursorIcon::ResizeNwSe
        }
        _ => CursorIcon::ResizeNeSw,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

/// Rounds the native window on Windows 10 (region fallback) and Windows 11 (DWM).
pub fn apply_rounded_corners(ctx: &egui::Context, maximized: bool) {
    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicI64, AtomicIsize, Ordering};
        static HWND: AtomicIsize = AtomicIsize::new(0);
        static SHAPE: AtomicI64 = AtomicI64::new(i64::MIN);

        let hwnd = {
            let cached = HWND.load(Ordering::Relaxed);
            if cached != 0 {
                cached as *mut core::ffi::c_void
            } else {
                let title: Vec<u16> = WINDOW_TITLE
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                let hwnd = unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
                        std::ptr::null(),
                        title.as_ptr(),
                    )
                };
                if hwnd.is_null() {
                    return;
                }
                HWND.store(hwnd as isize, Ordering::Relaxed);
                hwnd
            }
        };
        let ppp = ctx.pixels_per_point();
        let size = ctx.screen_rect().size();
        let w = (size.x * ppp).round() as i64;
        let h = (size.y * ppp).round() as i64;
        let shape = if maximized { -1 } else { (w << 32) | h };
        if SHAPE.swap(shape, Ordering::Relaxed) == shape {
            return;
        }
        if maximized {
            unsafe {
                windows_sys::Win32::Graphics::Gdi::SetWindowRgn(hwnd, std::ptr::null_mut(), 1);
            }
            return;
        }
        let radius = (theme::OUTER_RADIUS * ppp).round() as i32;
        let ellipse = (radius * 2).max(2);
        let region = unsafe {
            windows_sys::Win32::Graphics::Gdi::CreateRoundRectRgn(
                0,
                0,
                w as i32 + 1,
                h as i32 + 1,
                ellipse,
                ellipse,
            )
        };
        if !region.is_null() {
            unsafe {
                windows_sys::Win32::Graphics::Gdi::SetWindowRgn(hwnd, region, 1);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (ctx, maximized);
    }
}
