//! Native, transient QR rendering. No WebView, browser, avatar fetch or storage.
use crate::theme::{self, ButtonKind, Icon};
use bili_core::{auth::QrStatus, AuthSnapshot};
use egui::{Color32, RichText};
use std::time::{Duration, Instant};

pub enum LoginAction {
    Start { remember: bool },
    Cancel,
    Logout,
}

pub struct LoginView<'a> {
    pub auth: Option<&'a AuthSnapshot>,
    pub remembered: bool,
    pub available: bool,
    pub running: bool,
    pub cancelling: bool,
    pub reduced: bool,
}

#[derive(Default)]
pub struct LoginState {
    pub open: bool,
    pub error: Option<String>,
    texture: Option<egui::TextureHandle>,
    deadline: Option<Instant>,
    confirmation: bool,
    confirm_logout: bool,
    remember_choice: bool,
}

impl LoginState {
    pub fn update(&mut self, ctx: &egui::Context, snapshot: &AuthSnapshot) {
        self.confirmation = snapshot.qr_status == Some(QrStatus::WaitingForConfirmation);
        if let Some(challenge) = &snapshot.qr {
            self.deadline = Some(Instant::now() + Duration::from_secs(challenge.expires_in_secs));
            match qrcode::QrCode::new(challenge.login_url.as_bytes()) {
                Ok(code) => {
                    let quiet = 4;
                    let width = code.width();
                    let size = width + quiet * 2;
                    let mut image = egui::ColorImage::new([size, size], Color32::WHITE);
                    for y in 0..width {
                        for x in 0..width {
                            if code[(x, y)] == qrcode::Color::Dark {
                                image[(x + quiet, y + quiet)] = Color32::BLACK;
                            }
                        }
                    }
                    self.texture = Some(ctx.load_texture(
                        "transient-login-qr",
                        image,
                        egui::TextureOptions::NEAREST,
                    ));
                }
                Err(_) => self.error = Some("无法生成二维码，请取消后重试。".into()),
            }
        }
    }

    pub fn clear_challenge(&mut self) {
        self.texture = None;
        self.deadline = None;
        self.confirmation = false;
    }

    pub fn show(&mut self, ctx: &egui::Context, view: LoginView<'_>) -> Option<LoginAction> {
        let LoginView {
            auth,
            remembered,
            available,
            running,
            cancelling,
            reduced,
        } = view;
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut action = None;
        egui::Window::new("账号与登录")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .frame(theme::card_frame())
            .show(ctx, |ui| {
                theme::inset(theme::card()).show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(6.0);
                        if let Some(snapshot) = auth.filter(|a| a.logged_in) {
                            let name = snapshot.account.as_ref().map_or("Bilibili 用户", |a| a.name.as_str());
                            ui.label(RichText::new(name).size(23.0).strong());
                            theme::muted(
                                ui,
                                if remembered {
                                    "保持登录 · 下次启动自动恢复"
                                } else {
                                    "仅本次会话 · 关闭应用后需重新扫码"
                                },
                            );
                            ui.add_space(14.0);
                            theme::inset(theme::soft()).show(ui, |ui| {
                                theme::muted(
                                    ui,
                                    if remembered {
                                        "凭据保存在系统凭据库，不写入偏好文件；退出登录会同时删除。"
                                    } else {
                                        "凭据仅保留在内存中，关闭应用即清除。\n不会读取或修改 CLI 保存的账号。"
                                    },
                                );
                            });
                            if self.confirm_logout {
                                ui.add_space(6.0);
                                theme::inset(theme::rose()).show(ui, |ui| {
                                    ui.label(RichText::new("退出后需要重新扫码，确定退出吗？").strong());
                                    ui.horizontal_wrapped(|ui| {
                                        if theme::button(
                                            ui,
                                            "确认退出",
                                            Icon::User,
                                            ButtonKind::Danger,
                                            reduced,
                                        )
                                        .clicked()
                                        {
                                            self.confirm_logout = false;
                                            action = Some(LoginAction::Logout);
                                        }
                                        if theme::button(
                                            ui,
                                            "取消",
                                            Icon::Back,
                                            ButtonKind::Quiet,
                                            reduced,
                                        )
                                        .clicked()
                                        {
                                            self.confirm_logout = false;
                                        }
                                    });
                                });
                            } else {
                                ui.add_enabled_ui(available, |ui| {
                                    if theme::button(
                                        ui,
                                        "退出登录",
                                        Icon::User,
                                        ButtonKind::Quiet,
                                        reduced,
                                    )
                                    .clicked()
                                    {
                                        self.confirm_logout = true;
                                    }
                                });
                            }
                        } else {
                            ui.label(RichText::new("扫码，连接你的收藏").size(23.0).strong());
                            theme::muted(ui, "使用哔哩哔哩手机客户端扫码");
                            ui.add_space(12.0);
                            let expired = self.deadline.is_some_and(|d| Instant::now() >= d);
                            if let Some(texture) = self.texture.as_ref().filter(|_| !expired && !cancelling) {
                                theme::inset(theme::background()).show(ui, |ui| {
                                    ui.add(egui::Image::new((texture.id(), egui::vec2(232.0, 232.0))));
                                });
                            } else {
                                let (rect, _) = ui.allocate_exact_size(egui::vec2(232.0, 168.0), egui::Sense::hover());
                                ui.painter().rect_filled(rect, 16.0, theme::soft());
                                theme::icon(ui.painter(), egui::Rect::from_center_size(rect.center(), egui::vec2(48.0, 48.0)), Icon::User, theme::primary());
                            }
                            ui.add_space(8.0);
                            if running {
                                theme::loading(ui, reduced, if cancelling { "正在取消，请等待当前请求结束…" } else if expired { "二维码已过期，正在结束会话…" } else if self.confirmation { "已扫码，请在手机上确认" } else if self.texture.is_some() { "等待扫码" } else { "正在获取二维码…" });
                                if let Some(deadline) = self.deadline {
                                    theme::muted(ui, format!("有效期剩余 {} 秒", deadline.saturating_duration_since(Instant::now()).as_secs()));
                                    ctx.request_repaint_after(Duration::from_secs(1));
                                }
                                ui.add_enabled_ui(!cancelling, |ui| {
                                    if theme::button(ui, "取消登录", Icon::Close, ButtonKind::Quiet, reduced).clicked() { action = Some(LoginAction::Cancel); }
                                });
                            } else {
                                ui.add_enabled_ui(available, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        for (remember, name) in [
                                            (false, "仅本次登录"),
                                            (true, "保持登录状态"),
                                        ] {
                                            let selected = self.remember_choice == remember;
                                            if theme::button(
                                                ui,
                                                name,
                                                if selected { Icon::Check } else { Icon::User },
                                                if selected {
                                                    ButtonKind::Soft
                                                } else {
                                                    ButtonKind::Quiet
                                                },
                                                reduced,
                                            )
                                            .clicked()
                                            {
                                                self.remember_choice = remember;
                                                self.error = None;
                                            }
                                        }
                                    });
                                    theme::muted(
                                        ui,
                                        if self.remember_choice {
                                            "凭据写入系统凭据库，下次启动自动恢复，可随时退出。"
                                        } else {
                                            "凭据仅保留在本次会话，关闭应用后需重新扫码。"
                                        },
                                    );
                                    ui.add_space(6.0);
                                    if theme::button(
                                        ui,
                                        "获取登录二维码",
                                        Icon::User,
                                        ButtonKind::Primary,
                                        reduced,
                                    )
                                    .clicked()
                                    {
                                        self.error = None;
                                        self.confirm_logout = false;
                                        action = Some(LoginAction::Start {
                                            remember: self.remember_choice,
                                        });
                                    }
                                });
                            }
                            if let Some(error) = &self.error { ui.colored_label(theme::ERROR, error); }
                            ui.add_space(8.0);
                            ui.label(RichText::new("仅本次登录不写盘；保持登录写入系统凭据库，可随时退出。").size(12.0).color(theme::muted_color()));
                            ui.label(RichText::new("点击获取后才会连接登录服务；登录成功前仍以访客身份使用。").size(12.0).color(theme::muted_color()));
                        }
                    });
                });
            });
        if !open && running && !cancelling {
            action = Some(LoginAction::Cancel);
        }
        if !open {
            self.confirm_logout = false;
        }
        self.open = open;
        if matches!(action, Some(LoginAction::Cancel)) {
            self.clear_challenge();
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Rect, Vec2};

    fn texts(output: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn both_login_storage_choices_are_visible() {
        let ctx = egui::Context::default();
        let mut state = LoginState {
            open: true,
            ..LoginState::default()
        };
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 800.0))),
            ..Default::default()
        };
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(raw.clone(), |ctx| {
                let _ = state.show(
                    ctx,
                    LoginView {
                        auth: None,
                        remembered: false,
                        available: true,
                        running: false,
                        cancelling: false,
                        reduced: false,
                    },
                );
            }));
        }
        let all = texts(&output.unwrap());
        assert!(all.iter().any(|text| text == "仅本次登录"));
        assert!(all.iter().any(|text| text == "保持登录状态"));
        assert!(all.iter().any(|text| text == "获取登录二维码"));
        assert!(all.iter().any(|text| text.contains("凭据仅保留在本次会话")));
        assert!(all
            .iter()
            .any(|text| text.contains("保持登录写入系统凭据库")));
    }
}
