//! One configuration draft for detail and bulk entry; admitted receipts are immutable.
use crate::{
    backend::{self, AdmissionReceipt, AdmissionRequest, Operation, PrepareInput, PreparedItem},
    detail::part_label,
    search, theme,
};
use bili_core::{
    AuthSnapshot, BatchSettings, DownloadContext, DownloadSelection, MergePolicy, PartRef,
    QualityCatalog, SidecarPolicy, SourceProject, SubtitleSelection,
};
use egui::RichText;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
enum QualityChoice {
    Common,
    Exact(u32),
    Unavailable,
}
#[derive(Clone)]
struct Row {
    prepared: PreparedItem,
    selected: Vec<PartRef>,
    choice: QualityChoice,
    fulfilled: bool,
}
#[derive(Clone, Default, PartialEq, Eq)]
enum AccountChoice {
    #[default]
    Follow,
    Anonymous,
    Profile(String),
}
#[derive(Clone)]
pub struct ConfigState {
    pub open: bool,
    pub history_requested: bool,
    rows: Vec<Row>,
    pub directory: String,
    pub make_default: bool,
    rate_mib: f64,
    pub(crate) advanced_open: bool,
    subtitles: bool,
    danmaku: bool,
    quality: Option<u32>,
    account: AccountChoice,
    catalog_profile: Option<String>,
    batch_id: String,
    pub receipts: Vec<AdmissionReceipt>,
    pub error: Option<String>,
    pub verification: Option<bili_core::VerificationRequest>,
    result: Option<String>,
    admitted: Vec<PartRef>,
    intent: String,
}
impl Default for ConfigState {
    fn default() -> Self {
        Self {
            open: false,
            history_requested: false,
            rows: Vec::new(),
            directory: String::new(),
            make_default: false,
            rate_mib: 0.0,
            advanced_open: false,
            subtitles: true,
            danmaku: true,
            quality: None,
            account: AccountChoice::Follow,
            catalog_profile: None,
            batch_id: String::new(),
            receipts: Vec::new(),
            error: None,
            verification: None,
            result: None,
            admitted: Vec::new(),
            intent: String::new(),
        }
    }
}
impl ConfigState {
    pub fn key(&self) -> Option<String> {
        (!self.rows.is_empty()).then(|| self.intent.clone())
    }
    pub fn intent_key(items: &[PreparedItem]) -> String {
        let mut ids: Vec<_> = items
            .iter()
            .map(|item| {
                let mut parts: Vec<_> = item
                    .selected
                    .iter()
                    .map(|p| format!("{}:{}", p.aid, p.cid))
                    .collect();
                parts.sort();
                format!(
                    "{}:{}",
                    search::reference_input(&item.item.reference),
                    parts.join(",")
                )
            })
            .collect();
        ids.sort();
        ids.join("|")
    }
    pub fn matches_intent(&self, items: &[PreparedItem]) -> bool {
        self.intent == Self::intent_key(items)
            || self.rows.len() == items.len()
                && self.rows.iter().zip(items).all(|(row, item)| {
                    row.prepared.item.reference == item.item.reference && {
                        let selected: Vec<_> = item
                            .selected
                            .iter()
                            .filter(|p| !self.admitted.contains(p))
                            .collect();
                        selected.len() == row.selected.len()
                            && selected.iter().all(|p| row.selected.contains(p))
                    }
                })
    }
    pub fn accept(
        &mut self,
        items: Vec<PreparedItem>,
        profile: Option<String>,
        default: &std::path::Path,
        default_rate_mib: f64,
        refresh: bool,
    ) {
        let same = (refresh || self.matches_intent(&items))
            && self.rows.len() == items.len()
            && self
                .rows
                .iter()
                .zip(&items)
                .all(|(r, p)| r.prepared.item.reference == p.item.reference);
        if same {
            for (row, item) in self.rows.iter_mut().zip(items) {
                row.prepared = item;
            }
        } else {
            let intent = Self::intent_key(&items);
            *self = Self {
                directory: default.display().to_string(),
                rate_mib: crate::prefs::sanitize_download_rate(default_rate_mib),
                batch_id: format!("gui-{}", bili_core::JobId::new()),
                intent,
                ..Self::default()
            };
            self.rows = items
                .into_iter()
                .map(|prepared| Row {
                    selected: prepared.selected.clone(),
                    prepared,
                    choice: QualityChoice::Unavailable,
                    fulfilled: false,
                })
                .collect();
        }
        for index in 0..self.rows.len() {
            if self.rows[index].choice == QualityChoice::Unavailable {
                if let Some(quality) = self.preferred_quality(&self.rows[index]) {
                    self.rows[index].choice = QualityChoice::Exact(quality);
                }
            }
        }
        self.catalog_profile = profile;
        self.error = None;
        self.verification = self
            .rows
            .iter()
            .flat_map(|row| &row.prepared.catalogs)
            .filter_map(|(_, result)| result.as_ref().err())
            .find_map(|error| error.verification.clone());
        self.open = true;
    }
    pub fn exclude_admitted(&mut self, parts: Vec<PartRef>) {
        for row in &mut self.rows {
            if row.fulfilled && row.selected.is_empty() {
                // Only a freshly prepared intent can re-admit a removed job.
                // Preserve manual deselections and unfinished partial drafts.
                row.selected = row
                    .prepared
                    .selected
                    .iter()
                    .filter(|p| self.admitted.contains(p) && !parts.contains(p))
                    .cloned()
                    .collect();
                if !row.selected.is_empty() {
                    row.fulfilled = false;
                }
            }
            let had_admitted = row.selected.iter().any(|p| parts.contains(p));
            row.selected.retain(|p| !parts.contains(p));
            if had_admitted && row.selected.is_empty() {
                row.fulfilled = true;
            }
        }
        self.admitted = parts;
    }
    pub fn effective_profile(&self, auth: Option<&AuthSnapshot>) -> Option<String> {
        match &self.account {
            AccountChoice::Follow => auth.and_then(|a| a.selected_profile.clone()),
            AccountChoice::Anonymous => None,
            AccountChoice::Profile(id) => Some(id.clone()),
        }
    }
    fn quality_for(&self, row: &Row) -> Option<u32> {
        match row.choice {
            QualityChoice::Common => self.quality,
            QualityChoice::Exact(id) => Some(id),
            QualityChoice::Unavailable => None,
        }
    }
    #[cfg(test)]
    pub(crate) fn review_rate_override(&mut self, rate: f64) {
        self.advanced_open = true;
        self.set_rate_override(rate);
    }
    fn set_rate_override(&mut self, rate: f64) {
        self.rate_mib = crate::prefs::sanitize_download_rate(rate);
    }
    fn catalog<'a>(row: &'a Row, part: &PartRef) -> Result<&'a QualityCatalog, String> {
        row.prepared
            .catalogs
            .iter()
            .find(|(p, _)| p == part)
            .ok_or_else(|| "画质尚未检查。".into())
            .and_then(|(_, r)| r.as_ref().map_err(backend::error_message))
    }
    fn usable_qualities(&self, row: &Row) -> Vec<(u32, String)> {
        let Some(first) = row.selected.first() else {
            return Vec::new();
        };
        let Ok(catalog) = Self::catalog(row, first) else {
            return Vec::new();
        };
        let mut ids: Vec<_> = catalog
            .representations
            .iter()
            .filter(|r| !r.audio)
            .map(|r| r.quality)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.retain(|id| {
            row.selected.iter().all(|reference| {
                let Some(part) = row
                    .prepared
                    .item
                    .parts
                    .iter()
                    .find(|p| &p.reference == reference)
                else {
                    return false;
                };
                let Ok(catalog) = Self::catalog(row, reference) else {
                    return false;
                };
                let mut selection = self.selection(row, part);
                selection.quality = Some(*id);
                catalog.select(&selection).is_ok()
            })
        });
        ids.into_iter()
            .rev()
            .map(|id| (id, quality_label(id, &self.rows)))
            .collect()
    }
    fn usable_union(&self) -> Vec<(u32, String)> {
        let mut choices = Vec::new();
        for row in &self.rows {
            for choice in self.usable_qualities(row) {
                if !choices.iter().any(|(id, _)| *id == choice.0) {
                    choices.push(choice);
                }
            }
        }
        choices.sort_by_key(|(id, _)| std::cmp::Reverse(*id));
        choices
    }
    fn selected_dimensions(&self, row: &Row) -> Option<(u32, u32)> {
        let quality = self.quality_for(row)?;
        let mut dimensions = None;
        for reference in &row.selected {
            let part = row
                .prepared
                .item
                .parts
                .iter()
                .find(|p| &p.reference == reference)?;
            let catalog = Self::catalog(row, reference).ok()?;
            let mut selection = self.selection(row, part);
            selection.quality = Some(quality);
            let (video, _) = catalog.select(&selection).ok()?;
            let pair = (
                video.width.filter(|n| *n > 0)?,
                video.height.filter(|n| *n > 0)?,
            );
            if dimensions.is_some_and(|previous| previous != pair) {
                return None;
            }
            dimensions = Some(pair);
        }
        dimensions
    }
    fn preferred_quality(&self, row: &Row) -> Option<u32> {
        let choices = self.usable_qualities(row);
        let first = row.selected.first()?;
        let part = row
            .prepared
            .item
            .parts
            .iter()
            .find(|p| &p.reference == first)?;
        let mut catalog = Self::catalog(row, first).ok()?.clone();
        catalog
            .representations
            .retain(|r| r.audio || choices.iter().any(|(id, _)| *id == r.quality));
        let mut selection = self.selection(row, part);
        selection.quality = None;
        catalog
            .select(&selection)
            .ok()
            .map(|(video, _)| video.quality)
    }
    fn apply_common_quality(&mut self, quality: u32) {
        self.quality = Some(quality);
        for row in &mut self.rows {
            if !row.selected.is_empty() {
                row.choice = QualityChoice::Common;
            }
        }
    }
    fn row_error(&self, row: &Row) -> Option<String> {
        if row.selected.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        for reference in &row.selected {
            let catalog = match Self::catalog(row, reference) {
                Ok(catalog) => catalog,
                Err(error) => return Some(error),
            };
            let Some(part) = row
                .prepared
                .item
                .parts
                .iter()
                .find(|part| &part.reference == reference)
            else {
                return Some("分集已变化，请重新选择。".into());
            };
            parts.push((part, catalog));
        }
        let Some(quality) = self.quality_for(row) else {
            return Some("没有可下载的画质，请重新检查内容或账号。".into());
        };
        for (part, catalog) in parts {
            if catalog.select(&self.selection(row, part)).is_err() {
                return Some(format!(
                    "第 {} 项没有可获取的 {}；请选择其他画质或取消勾选此项。",
                    part.ordinal,
                    quality_label(quality, &self.rows)
                ));
            }
        }
        None
    }
    fn account_error(&self, auth: Option<&AuthSnapshot>) -> Option<String> {
        let id = self.effective_profile(auth)?;
        match auth.and_then(|a| a.profiles.iter().find(|p| p.id == id)) {
            None => Some("账号凭据已移除，请重新选择。".into()),
            Some(profile) if profile.validity == bili_core::auth::ProfileValidity::Expired => {
                Some("账号凭据已过期，请重新登录。".into())
            }
            _ => None,
        }
    }
    fn can_submit(&self, auth: Option<&AuthSnapshot>) -> bool {
        self.rows.iter().any(|r| !r.selected.is_empty())
            && self.rows.iter().all(|r| self.row_error(r).is_none())
            && self.account_error(auth).is_none()
            && self.effective_profile(auth) == self.catalog_profile
            && PathBuf::from(&self.directory).is_absolute()
            && self.rate_mib.is_finite()
            && self.rate_mib >= 0.0
    }
    pub fn acknowledge(
        &mut self,
        receipts: Vec<AdmissionReceipt>,
        cancelled: bool,
        error: Option<String>,
    ) {
        let parts: Vec<PartRef> = receipts.iter().map(|r| r.plan.part.clone()).collect();
        for row in &mut self.rows {
            let had_admitted = row.selected.iter().any(|p| parts.contains(p));
            row.selected.retain(|p| !parts.contains(p));
            if had_admitted && row.selected.is_empty() {
                row.fulfilled = true;
            }
        }
        self.admitted.extend(parts);
        let count = receipts.len();
        for receipt in receipts {
            if !self.receipts.iter().any(|r| r.id == receipt.id) {
                self.receipts.push(receipt);
            }
        }
        self.result = Some(format!(
            "已添加 {count} 项。{}",
            if cancelled {
                "其余内容保留，可继续添加。"
            } else {
                ""
            }
        ));
        self.error = error;
    }
    pub fn inputs(&self) -> Vec<PrepareInput> {
        self.rows
            .iter()
            .map(|r| PrepareInput {
                input: search::reference_input(&r.prepared.item.reference),
                parts: Some(r.selected.clone()),
                category: r.prepared.category,
            })
            .collect()
    }
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        available: bool,
        auth: Option<&AuthSnapshot>,
    ) -> Option<Operation> {
        if !self.open {
            return None;
        }
        let bounds = dialog_bounds(ctx.screen_rect());
        let inner = (bounds.size() - egui::Vec2::splat(32.0)).max(egui::Vec2::splat(1.0));
        let stacked_footer = inner.x < 500.0 && !self.receipts.is_empty();
        let footer_rows = if stacked_footer { 2.0 } else { 1.0 };
        let mut action = None;
        egui::Window::new("下载配置")
            .id(egui::Id::new("download-config"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .frame(egui::Frame::window(&ctx.style()).inner_margin(16.0))
            .fixed_pos(bounds.min)
            .fixed_size(inner)
            .min_size(egui::Vec2::ZERO)
            .max_size(inner)
            .constrain_to(bounds)
            .show(ctx, |ui| {
                ui.set_width(inner.x);
                let button_height = ui
                    .fonts(|fonts| fonts.row_height(&egui::TextStyle::Button.resolve(ui.style())))
                    .ceil()
                    + 2.0 * ui.spacing().button_padding.y;
                let row_height = button_height.max(ui.spacing().interact_size.y);
                let header = ui.vertical(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(inner.x, 28.0),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if theme::close_button(ui).clicked() {
                                self.open = false;
                            }
                        },
                    );
                    ui.add_enabled_ui(available, |ui| {
                        ui.label("保存目录（可直接编辑或选择文件夹）");
                        ui.horizontal(|ui| {
                            let picker = egui::WidgetText::from("选择目录").into_galley(
                                ui,
                                Some(egui::TextWrapMode::Extend),
                                f32::INFINITY,
                                egui::TextStyle::Button,
                            );
                            let picker_width = (picker.size().x
                                + 2.0 * ui.spacing().button_padding.x)
                                .max(ui.spacing().interact_size.x);
                            let height = ui.spacing().interact_size.y;
                            let width =
                                (ui.available_width() - picker_width - ui.spacing().item_spacing.x)
                                    .max(0.0);
                            ui.add_sized(
                                [width, height],
                                theme::singleline_input(
                                    ui,
                                    &mut self.directory,
                                    "输入绝对路径",
                                    height,
                                ),
                            );
                            if ui.button(picker).clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .set_directory(&self.directory)
                                    .pick_folder()
                                {
                                    self.directory = path.display().to_string();
                                }
                            }
                        });
                        ui.checkbox(&mut self.make_default, "将此目录设为默认保存位置");
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut self.subtitles, "字幕");
                            ui.checkbox(&mut self.danmaku, "弹幕");
                        });
                    });
                    ui.separator();
                });
                // A Window can offer an unbounded available height while measuring its
                // contents. Budget the scroll body from the finite viewport instead so
                // the close row and admission controls always remain inside the frame.
                let footer_height =
                    row_height * footer_rows + ui.spacing().item_spacing.y * (footer_rows - 1.0);
                let body_height = (inner.y
                    - header.response.rect.height()
                    - footer_height
                    - 2.0 * ui.spacing().item_spacing.y)
                    .max(0.0);

                egui::ScrollArea::vertical()
                    .id_salt("download-config-body")
                    .auto_shrink([false, false])
                    .max_height(body_height)
                    .show(ui, |ui| {
                        if let Some(result) = &self.result {
                            ui.label(result);
                        }
                        if let Some(error) = &self.error {
                            ui.colored_label(theme::error_color(), error);
                        }
                        if let Some(request) = self.verification.clone() {
                            if ui
                                .add_enabled(available, egui::Button::new("安全验证"))
                                .clicked()
                            {
                                action = Some(Operation::BeginVerification {
                                    request,
                                    profile: self.effective_profile(auth),
                                });
                            }
                        }
                        if let Some(error) = self.account_error(auth) {
                            ui.colored_label(theme::error_color(), error);
                        }
                        ui.add_enabled_ui(available, |ui| {
                            if self.rows.len() > 1 {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label("批量画质");
                                    let choices = self.usable_union();
                                    let mut selected = None;
                                    egui::ComboBox::from_id_salt("common-quality")
                                        .width(170.0)
                                        .height(320.0)
                                        .selected_text(self.quality.map_or_else(
                                            || "选择画质".into(),
                                            |id| quality_label(id, &self.rows),
                                        ))
                                        .show_ui(ui, |ui| {
                                            for (id, label) in choices {
                                                if ui
                                                    .selectable_label(
                                                        self.quality == Some(id),
                                                        label,
                                                    )
                                                    .clicked()
                                                {
                                                    selected = Some(id);
                                                }
                                            }
                                        });
                                    if let Some(quality) = selected {
                                        self.apply_common_quality(quality);
                                    }
                                });
                            }
                        });
                        let profile = self.effective_profile(auth);
                        let profile_stale = profile != self.catalog_profile;
                        if profile_stale {
                            ui.colored_label(
                                theme::error_color(),
                                "账号已变化，请在高级设置中重新检查画质。",
                            );
                        }
                        ui.separator();
                        for index in 0..self.rows.len() {
                            let error = self.row_error(&self.rows[index]);
                            let choices = self.usable_qualities(&self.rows[index]);
                            let selected_quality = self.quality_for(&self.rows[index]);
                            let dimensions = self.selected_dimensions(&self.rows[index]);
                            let selected_text = selected_quality.map_or_else(
                                || "没有可用画质".into(),
                                |quality| quality_label(quality, &self.rows),
                            );
                            let row = &mut self.rows[index];
                            let frame = egui::Frame::group(ui.style()).stroke(egui::Stroke::new(
                                1.0_f32,
                                if error.is_some() {
                                    theme::error_color()
                                } else {
                                    theme::border()
                                },
                            ));
                            frame.show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(&row.prepared.item.title).strong());
                                });
                                ui.add_enabled_ui(available, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label("源画质");
                                        egui::ComboBox::from_id_salt(("row-quality", index))
                                            .width(170.0)
                                            .height(320.0)
                                            .selected_text(selected_text)
                                            .show_ui(ui, |ui| {
                                                for (id, label) in choices {
                                                    if ui
                                                        .selectable_label(
                                                            selected_quality == Some(id),
                                                            label,
                                                        )
                                                        .clicked()
                                                    {
                                                        row.choice = QualityChoice::Exact(id);
                                                    }
                                                }
                                            });
                                        if let Some((width, height)) = dimensions {
                                            theme::muted(
                                                ui,
                                                format!("视频尺寸：{width} × {height}"),
                                            );
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        if ui.small_button("全选").clicked() {
                                            row.selected = row
                                                .prepared
                                                .item
                                                .parts
                                                .iter()
                                                .filter(|p| !self.admitted.contains(&p.reference))
                                                .map(|p| p.reference.clone())
                                                .collect();
                                        }
                                        if ui.small_button("清空").clicked() {
                                            row.selected.clear();
                                        }
                                        theme::muted(
                                            ui,
                                            format!(
                                                "已选 {} / {}",
                                                row.selected.len(),
                                                row.prepared.item.parts.len()
                                            ),
                                        );
                                    });
                                    ui.horizontal_wrapped(|ui| {
                                        for part in &row.prepared.item.parts {
                                            let mut selected =
                                                row.selected.contains(&part.reference);
                                            if ui
                                                .add_enabled(
                                                    !self.admitted.contains(&part.reference),
                                                    egui::Checkbox::new(
                                                        &mut selected,
                                                        format!("{}  {}", part.ordinal, part.title),
                                                    ),
                                                )
                                                .changed()
                                            {
                                                if selected {
                                                    row.selected.push(part.reference.clone());
                                                } else {
                                                    row.selected.retain(|p| p != &part.reference);
                                                }
                                            }
                                        }
                                    });
                                });
                                if let Some(error) = error {
                                    ui.colored_label(theme::error_color(), error);
                                }
                            });
                            ui.add_space(6.0);
                        }
                        if let Some(operation) = self.advanced_options(ui, available, auth) {
                            action = Some(operation);
                        }
                        if !self.receipts.is_empty() {
                            ui.separator();
                            ui.label("已添加");
                            for receipt in &self.receipts {
                                ui.label(format!(
                                    "{} · {} · {}",
                                    receipt.plan.title,
                                    receipt.plan.video.codec,
                                    receipt.plan.destination.display()
                                ));
                                theme::muted(
                                    ui,
                                    format!("{} · 画质 {}", receipt.id, receipt.plan.video.quality),
                                );
                            }
                        }
                    });
                let count = self.rows.iter().map(|r| r.selected.len()).sum::<usize>();
                let blocked = !self.can_submit(auth);
                ui.allocate_ui_with_layout(
                    egui::vec2(inner.x, footer_height),
                    if stacked_footer {
                        egui::Layout::top_down(egui::Align::Min)
                    } else {
                        egui::Layout::left_to_right(egui::Align::Center)
                    },
                    |ui| {
                        if !self.receipts.is_empty() && ui.button("查看下载").clicked() {
                            self.history_requested = true;
                        }
                        if ui
                            .add_enabled(
                                available && count > 0 && !blocked,
                                egui::Button::new(format!("开始下载 · {count} 项")),
                            )
                            .clicked()
                        {
                            self.batch_id = format!("gui-{}", bili_core::JobId::new());
                            let selections = self.requests(auth);
                            self.result = None;
                            self.error = None;
                            action = Some(Operation::Enqueue {
                                selections,
                                output_dir: PathBuf::from(&self.directory),
                            });
                        }
                    },
                );
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open = false;
        }
        action
    }
    fn advanced_options(
        &mut self,
        ui: &mut egui::Ui,
        available: bool,
        auth: Option<&AuthSnapshot>,
    ) -> Option<Operation> {
        let advanced = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let icon_width = ui.spacing().icon_width;
                let (icon_rect, icon_response) = ui.allocate_exact_size(
                    egui::vec2(icon_width, ui.spacing().interact_size.y),
                    egui::Sense::click(),
                );
                let response =
                    icon_response.union(ui.selectable_label(self.advanced_open, "高级设置"));
                egui::collapsing_header::paint_default_icon(
                    ui,
                    if self.advanced_open { 1.0 } else { 0.0 },
                    &response.clone().with_new_rect(egui::Rect::from_center_size(
                        icon_rect.center(),
                        egui::Vec2::splat(icon_width),
                    )),
                );
                response
            })
            .inner;
        if advanced.clicked() {
            self.advanced_open = !self.advanced_open;
        }
        let mut action = None;
        if self.advanced_open {
            let options = ui.add_enabled_ui(available, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("本次下载限速");
                    let mut rate = self.rate_mib;
                    if ui
                        .add(
                            egui::DragValue::new(&mut rate)
                                .range(0.0..=crate::prefs::MAX_DOWNLOAD_RATE_MIB)
                                .speed(0.1)
                                .suffix(" MiB/s"),
                        )
                        .changed()
                    {
                        self.set_rate_override(rate);
                    }
                    theme::muted(ui, "0 不限速");
                });
                let previous = self.account.clone();
                egui::ComboBox::from_id_salt("batch-account")
                    .selected_text(match &self.account {
                        AccountChoice::Follow => "跟随当前账号".into(),
                        AccountChoice::Anonymous => "匿名".into(),
                        AccountChoice::Profile(id) => profile_label(id, auth),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.account,
                            AccountChoice::Follow,
                            "跟随当前账号",
                        );
                        ui.selectable_value(&mut self.account, AccountChoice::Anonymous, "匿名");
                        if let Some(auth) = auth {
                            for profile in &auth.profiles {
                                ui.selectable_value(
                                    &mut self.account,
                                    AccountChoice::Profile(profile.id.clone()),
                                    &profile.account.name,
                                );
                            }
                        }
                    });
                let recheck = ui.button("重新检查画质").clicked();
                if self.account != previous || recheck {
                    action = Some(Operation::Prepare {
                        refresh: true,
                        inputs: self.inputs(),
                        profile: self.effective_profile(auth),
                    });
                }
            });
            if advanced.clicked() {
                ui.scroll_to_rect_animation(
                    advanced.rect.union(options.response.rect),
                    Some(egui::Align::Min),
                    egui::style::ScrollAnimation::none(),
                );
            }
        }
        action
    }
    fn selection(&self, row: &Row, part: &bili_core::Part) -> DownloadSelection {
        DownloadSelection {
            part: part.reference.clone(),
            title: part_label(&row.prepared.item.title, &part.title),
            quality: self.quality_for(row),
            video_representation: None,
            audio_representation: None,
            codec: None,
            dynamic_range: None,
            output: None,
            merge: MergePolicy::Automatic,
            sidecars: SidecarPolicy {
                subtitles: if self.subtitles {
                    SubtitleSelection::All
                } else {
                    SubtitleSelection::None
                },
                danmaku: self.danmaku,
            },
            keep_temp: false,
        }
    }
    fn requests(&self, auth: Option<&AuthSnapshot>) -> Vec<AdmissionRequest> {
        let profile_id = self.effective_profile(auth);
        let limit = (self.rate_mib > 0.0).then_some((self.rate_mib * 1048576.0) as u64);
        self.rows
            .iter()
            .flat_map(|row| {
                row.prepared
                    .item
                    .parts
                    .iter()
                    .filter(|p| row.selected.contains(&p.reference))
                    .map(|part| AdmissionRequest {
                        selection: self.selection(row, part),
                        context: DownloadContext {
                            project: Some(SourceProject {
                                reference: row.prepared.item.source.as_ref().map_or_else(
                                    || row.prepared.item.reference.clone(),
                                    |s| s.reference.clone(),
                                ),
                                title: row.prepared.item.source.as_ref().map_or_else(
                                    || row.prepared.item.title.clone(),
                                    |s| s.title.clone(),
                                ),
                                category: (row.prepared.category != bili_core::SearchCategory::All)
                                    .then_some(row.prepared.category)
                                    .or_else(|| {
                                        row.prepared.item.source.as_ref().and_then(|s| s.category)
                                    }),
                                cover: row.prepared.item.cover.clone(),
                                ordinal: part.ordinal,
                            }),
                            batch: Some(BatchSettings {
                                id: self.batch_id.clone(),
                                rate_limit: limit,
                            }),
                            profile_id: profile_id.clone(),
                        },
                    })
            })
            .collect()
    }
}
fn dialog_bounds(screen: egui::Rect) -> egui::Rect {
    egui::Rect::from_center_size(
        screen.center(),
        egui::vec2(
            (screen.width() * 0.8).clamp(1.0, 720.0),
            (screen.height() * 0.8).clamp(1.0, 720.0),
        ),
    )
}
fn quality_label(id: u32, rows: &[Row]) -> String {
    for row in rows {
        for (_, result) in &row.prepared.catalogs {
            if let Ok(catalog) = result {
                if let Some(entry) = catalog
                    .advertised
                    .iter()
                    .find(|entry| entry.id == id && !entry.label.trim().is_empty())
                {
                    return entry.label.clone();
                }
            }
        }
    }
    for row in rows {
        for (_, result) in &row.prepared.catalogs {
            if let Ok(catalog) = result {
                if let Some(video) = catalog
                    .representations
                    .iter()
                    .find(|representation| !representation.audio && representation.quality == id)
                {
                    match (video.width, video.height) {
                        (Some(width), Some(height)) if width > 0 && height > 0 => {
                            return format!("{width} × {height}");
                        }
                        (_, Some(height)) if height > 0 => return format!("{height}P"),
                        _ => {}
                    }
                }
            }
        }
    }
    format!("画质 {id}")
}
fn profile_label(id: &str, auth: Option<&AuthSnapshot>) -> String {
    auth.and_then(|a| a.profiles.iter().find(|p| p.id == id))
        .map_or_else(
            || "账号已移除，需要重新选择".into(),
            |p| p.account.name.clone(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bili_core::{
        Aid, Availability, Cid, ContentRef, JobId, MediaItem, Part, QualityEntry,
        RepresentationKey, SearchCategory, StreamPlan, StreamTopology,
    };
    fn prepared(id: u64, qualities: &[u32]) -> PreparedItem {
        let aid = Aid::new(id).unwrap();
        let part = PartRef {
            content: ContentRef::Video(aid),
            aid,
            cid: Cid::new(id + 100).unwrap(),
            episode: None,
        };
        let catalog = QualityCatalog {
            part: part.clone(),
            advertised: qualities
                .iter()
                .map(|q| QualityEntry {
                    id: *q,
                    label: match *q {
                        120 => "4K",
                        112 => "1080P 高码率",
                        80 => "1080P",
                        64 => "720P",
                        _ => "其他画质",
                    }
                    .into(),
                    rank: *q,
                    availability: Availability::Obtainable,
                    requires_login: None,
                    requires_vip: None,
                    has_video_url: true,
                    advertised_codecs: vec!["avc1".into()],
                })
                .collect(),
            representations: qualities
                .iter()
                .map(|q| RepresentationKey {
                    part: part.clone(),
                    id: u64::from(*q),
                    quality: *q,
                    codec: "avc1".into(),
                    width: Some(if *q >= 120 {
                        3840
                    } else if *q >= 80 {
                        1920
                    } else {
                        1280
                    }),
                    height: Some(if *q >= 120 {
                        2160
                    } else if *q >= 80 {
                        1080
                    } else {
                        720
                    }),
                    fps: None,
                    dynamic_range: None,
                    color: None,
                    bandwidth: u64::from(*q) * 1000,
                    audio: false,
                    sample_rate: None,
                    channels: None,
                    language: None,
                })
                .collect(),
            revision: 1,
            preview: false,
        };
        PreparedItem {
            item: MediaItem {
                reference: ContentRef::Video(aid),
                bvid: None,
                title: format!("视频 {id}"),
                cover: None,
                duration_ms: Some(60000),
                parts: vec![Part {
                    reference: part.clone(),
                    ordinal: 3,
                    title: "P3".into(),
                    duration_ms: Some(60000),
                    preview: false,
                }],
                source: None,
                description: None,
                author: None,
            },
            category: SearchCategory::Video,
            selected: vec![part.clone()],
            catalogs: vec![(part, Ok(catalog))],
        }
    }
    fn receipt(request: &AdmissionRequest, destination: PathBuf) -> AdmissionReceipt {
        let video = prepared(
            request.selection.part.aid.get(),
            &[request.selection.quality.unwrap_or(64)],
        )
        .catalogs[0]
            .1
            .as_ref()
            .unwrap()
            .representations[0]
            .clone();
        AdmissionReceipt {
            id: JobId::new(),
            plan: StreamPlan {
                schema_version: 1,
                fingerprint: "synthetic-ui-receipt".into(),
                part: request.selection.part.clone(),
                title: request.selection.title.clone(),
                video,
                audio: None,
                topology: StreamTopology::Dash,
                segments: vec![],
                destination,
                merge: MergePolicy::Automatic,
                sidecars: request.selection.sidecars.clone(),
                keep_temp: false,
                catalog_revision: 1,
                context: Some(request.context.clone()),
            },
        }
    }
    #[test]
    fn strict_common_quality_blocks_only_incompatible_rows_and_requires_explicit_choice() {
        let root = std::env::temp_dir().join("bili-config-tests");
        let mut config = ConfigState::default();
        config.accept(
            vec![prepared(1, &[64]), prepared(2, &[112, 80, 64])],
            None,
            &root,
            0.0,
            false,
        );
        config.apply_common_quality(112);
        assert!(config.row_error(&config.rows[0]).is_some());
        assert!(config.row_error(&config.rows[1]).is_none());
        assert!(!config.can_submit(None));
        config.rows[0].choice = QualityChoice::Exact(64);
        assert!(config.can_submit(None));
        let requests = config.requests(None);
        assert_eq!(requests[0].selection.quality, Some(64));
        assert_eq!(requests[1].selection.quality, Some(112));
        assert_eq!(requests[0].context.project.as_ref().unwrap().ordinal, 3);
        config.rows[0].choice = QualityChoice::Exact(80);
        assert!(!config.can_submit(None));
        config.rows[0].selected.clear();
        assert!(config.can_submit(None));
        assert_eq!(config.requests(None).len(), 1);
    }
    #[test]
    fn preview_and_permission_metadata_use_core_obtainability_before_admission() {
        let root = std::env::temp_dir().join("bili-config-access");
        let mut item = prepared(1, &[112, 64]);
        item.catalogs[0].1.as_mut().unwrap().advertised[0].availability =
            Availability::EntitlementRequired;
        let mut config = ConfigState::default();
        config.accept(vec![item], None, &root, 0.0, false);
        config.apply_common_quality(112);
        assert!(
            !config.can_submit(None),
            "metadata alone must not make a gated representation selectable"
        );
        assert!(config.row_error(&config.rows[0]).is_some());
        config.rows[0].choice = QualityChoice::Exact(64);
        assert!(
            config.can_submit(None),
            "an explicit obtainable override is allowed"
        );
        config.rows[0].prepared.catalogs[0]
            .1
            .as_mut()
            .unwrap()
            .preview = true;
        assert!(
            !config.can_submit(None),
            "preview-only catalogs must remain blocked"
        );
        config.rows[0].prepared.catalogs[0]
            .1
            .as_mut()
            .unwrap()
            .preview = false;
        assert!(config.can_submit(None));
    }
    #[test]
    fn widescreen_source_tier_and_pixel_dimensions_are_displayed_independently() {
        let root = std::env::temp_dir().join("bili-widescreen-config");
        let mut item = prepared(1, &[80, 64]);
        item.catalogs[0].1.as_mut().unwrap().representations[0].height = Some(804);
        let mut config = ConfigState::default();
        config.accept(vec![item], None, &root, 0.0, false);
        assert_eq!(config.quality_for(&config.rows[0]), Some(80));
        assert_eq!(
            config.selected_dimensions(&config.rows[0]),
            Some((1920, 804))
        );
        assert!(config.can_submit(None));
        assert_eq!(config.requests(None)[0].selection.quality, Some(80));
        let ctx = egui::Context::default();
        theme::apply_style(&ctx, false);
        let size = egui::vec2(1180.0, 800.0);
        let _ = frame(&ctx, &mut config, size, vec![]);
        let output = frame(&ctx, &mut config, size, vec![]);
        let labels = texts(&output);
        assert!(labels.iter().any(|(text, _)| text == "源画质"));
        assert!(labels.iter().any(|(text, _)| text == "1080P"));
        assert!(labels
            .iter()
            .any(|(text, _)| text == "视频尺寸：1920 × 804"));
    }

    #[test]
    fn a_removed_acknowledged_part_can_be_deliberately_prepared_again() {
        let root = std::env::temp_dir().join("bili-readmit-tests");
        let mut config = ConfigState::default();
        let items = vec![prepared(1, &[64]), prepared(2, &[64])];
        config.accept(items.clone(), None, &root, 0.0, false);
        let request = config.requests(None).remove(0);
        let part = request.selection.part.clone();
        config.acknowledge(
            vec![receipt(&request, root.join("original.mp4"))],
            false,
            None,
        );
        let original = serde_json::to_value(&config.receipts[0].plan).unwrap();
        config.rows[1].selected.clear();
        config.rate_mib = 10.0;
        config.directory = root.join("chosen").display().to_string();
        config.accept(items.clone(), None, &root, 0.0, false);
        config.exclude_admitted(vec![part.clone()]);
        assert!(config.rows[0].selected.is_empty());
        assert!(config.rows[0].fulfilled);
        config.accept(items, None, &root, 0.0, false);
        config.exclude_admitted(vec![]);
        assert_eq!(config.rows[0].selected, vec![part]);
        assert!(!config.rows[0].fulfilled);
        assert!(
            config.rows[1].selected.is_empty(),
            "manual deselection survives"
        );
        assert_eq!(config.rate_mib, 10.0);
        assert_eq!(config.directory, root.join("chosen").display().to_string());
        assert_eq!(
            serde_json::to_value(&config.receipts[0].plan).unwrap(),
            original
        );
    }

    #[test]
    fn acknowledged_receipts_remain_immutable_and_manual_empty_selection_is_not_fulfilled() {
        let root = std::env::temp_dir().join("bili-receipt-tests");
        let mut config = ConfigState::default();
        let items = vec![prepared(1, &[64]), prepared(2, &[64])];
        config.accept(items.clone(), None, &root, 0.0, false);
        let request = config.requests(None).remove(0);
        let admitted = receipt(&request, root.join("original.mp4"));
        let original = serde_json::to_value(&admitted.plan).unwrap();
        config.acknowledge(vec![admitted], false, Some("第二项添加失败".into()));
        assert!(config.rows[0].fulfilled);
        assert!(config.row_error(&config.rows[0]).is_none());
        assert_eq!(config.requests(None).len(), 1);
        config.rows[1].selected.clear();
        assert!(!config.can_submit(None));
        assert!(!config.rows[1].fulfilled);
        config.directory = std::env::temp_dir()
            .join("retry-root")
            .display()
            .to_string();
        config.rate_mib = 4.5;
        config.apply_common_quality(112);
        assert_eq!(
            serde_json::to_value(&config.receipts[0].plan).unwrap(),
            original
        );
        config.open = false;
        config.accept(items, None, &root, 0.0, false);
        assert_eq!(config.rate_mib, 4.5);
        assert!(config.rows[1].selected.is_empty());
        assert_eq!(
            config.directory,
            std::env::temp_dir()
                .join("retry-root")
                .display()
                .to_string()
        );
    }
    #[test]
    fn a_new_part_selection_is_new_intent_but_catalog_refresh_preserves_the_draft() {
        let root = std::env::temp_dir().join("bili-part-intent");
        let mut first = prepared(1, &[64]);
        let mut second = first.item.parts[0].clone();
        second.reference.cid = Cid::new(900).unwrap();
        second.ordinal = 4;
        first.item.parts.push(second.clone());
        let mut config = ConfigState::default();
        config.accept(vec![first.clone()], None, &root, 0.0, false);
        config.directory = std::env::temp_dir()
            .join("deliberate-draft")
            .display()
            .to_string();
        config.rate_mib = 2.0;
        config.accept(vec![first.clone()], None, &root, 0.0, true);
        assert_eq!(config.rate_mib, 2.0);
        let mut changed = first;
        changed.selected = vec![second.reference.clone()];
        assert!(!config.matches_intent(std::slice::from_ref(&changed)));
        config.accept(vec![changed], None, &root, 0.0, false);
        assert_eq!(config.rows[0].selected, vec![second.reference]);
        assert_eq!(config.directory, root.display().to_string());
        assert_eq!(config.rate_mib, 0.0);
    }
    #[test]
    fn advertised_only_quality_is_absent_and_single_content_defaults_to_concrete_obtainable_quality(
    ) {
        let root = std::env::temp_dir().join("bili-usable-quality");
        let mut item = prepared(1, &[80, 64]);
        let mut advertised = item.catalogs[0].1.as_ref().unwrap().advertised[0].clone();
        advertised.id = 120;
        advertised.label = "4K".into();
        advertised.rank = 120;
        advertised.has_video_url = false;
        item.catalogs[0]
            .1
            .as_mut()
            .unwrap()
            .advertised
            .push(advertised);
        let mut config = ConfigState::default();
        config.accept(vec![item], None, &root, 0.0, false);
        assert_eq!(config.quality_for(&config.rows[0]), Some(80));
        assert_eq!(
            config
                .usable_qualities(&config.rows[0])
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            vec![80, 64]
        );
        assert!(config.can_submit(None));
        let ctx = egui::Context::default();
        theme::apply_style(&ctx, false);
        let size = egui::vec2(1180.0, 800.0);
        let _ = frame(&ctx, &mut config, size, vec![]);
        let output = frame(&ctx, &mut config, size, vec![]);
        let labels = texts(&output);
        assert_eq!(labels.iter().filter(|(text, _)| text == "1080P").count(), 1);
        assert!(!labels.iter().any(|(text, _)| text.contains("最高")
            || text.contains("批量画质")
            || text == "下载配置"));
        click(&ctx, &mut config, size, "1080P");
        let output = frame(&ctx, &mut config, size, vec![]);
        let labels = texts(&output);
        assert!(labels.iter().any(|(text, _)| text == "720P"));
        assert!(!labels.iter().any(|(text, _)| text == "4K"));
        config.rows[0].prepared.catalogs[0]
            .1
            .as_mut()
            .unwrap()
            .advertised
            .clear();
        assert_eq!(quality_label(80, &config.rows), "1920 × 1080");
    }
    #[test]
    fn available_intersection_and_account_refresh_never_silently_replace_an_explicit_quality() {
        let root = std::env::temp_dir().join("bili-quality-refresh");
        let item = prepared(1, &[120, 80, 64]);
        let mut config = ConfigState::default();
        config.accept(vec![item.clone()], None, &root, 0.0, false);
        config.rows[0].choice = QualityChoice::Exact(120);
        let mut gated = item.clone();
        gated.catalogs[0].1.as_mut().unwrap().advertised[0].availability =
            Availability::EntitlementRequired;
        config.accept(vec![gated], None, &root, 0.0, true);
        assert_eq!(config.quality_for(&config.rows[0]), Some(120));
        assert!(config.row_error(&config.rows[0]).is_some());
        assert_eq!(
            config
                .usable_qualities(&config.rows[0])
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            vec![80, 64]
        );
        let mut multipart = prepared(2, &[80, 64]);
        let mut part = multipart.item.parts[0].clone();
        part.reference.cid = Cid::new(900).unwrap();
        part.ordinal = 4;
        let mut lower = prepared(3, &[64]).catalogs.remove(0).1.unwrap();
        lower.part = part.reference.clone();
        for representation in &mut lower.representations {
            representation.part = part.reference.clone();
        }
        multipart.selected.push(part.reference.clone());
        multipart.catalogs.push((part.reference.clone(), Ok(lower)));
        multipart.item.parts.push(part);
        config.accept(vec![multipart], None, &root, 0.0, false);
        assert_eq!(
            config
                .usable_qualities(&config.rows[0])
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            vec![64]
        );
        assert_eq!(config.quality_for(&config.rows[0]), Some(64));
    }
    #[test]
    fn catalog_failures_keep_the_actionable_reason_even_without_a_concrete_quality() {
        let root = std::env::temp_dir().join("bili-catalog-failure");
        let mut item = prepared(1, &[80]);
        let error = bili_core::CoreError::new(bili_core::ErrorCode::Transport, "fixture catalog");
        let message = backend::error_message(&error);
        item.catalogs[0].1 = Err(error);
        let mut config = ConfigState::default();
        config.accept(vec![item], None, &root, 0.0, false);
        assert_eq!(config.row_error(&config.rows[0]), Some(message));
        assert!(!config.can_submit(None));
        let mut multipart = prepared(2, &[80]);
        let mut part = multipart.item.parts[0].clone();
        part.reference.cid = Cid::new(901).unwrap();
        part.ordinal = 4;
        multipart.selected.push(part.reference.clone());
        multipart.item.parts.push(part.clone());
        let error = bili_core::CoreError::new(
            bili_core::ErrorCode::LoginRequired,
            "fixture second catalog",
        );
        let expected = backend::error_message(&error);
        multipart.catalogs.push((part.reference, Err(error)));
        config.accept(vec![multipart], None, &root, 0.0, false);
        assert_eq!(config.row_error(&config.rows[0]), Some(expected));
    }
    #[test]
    fn new_drafts_inherit_the_global_rate_but_overrides_and_receipts_remain_local() {
        let root = std::env::temp_dir().join("bili-rate-default");
        let prefs = crate::prefs::GuiPrefs {
            default_download_rate_mib: 10.0,
            ..Default::default()
        };
        let first = prepared(1, &[80]);
        let mut config = ConfigState::default();
        config.accept(
            vec![first.clone()],
            None,
            &root,
            prefs.default_download_rate_mib,
            false,
        );
        assert_eq!(config.rate_mib, 10.0);
        assert!(!config.advanced_open);
        config.review_rate_override(100.0);
        let request = config.requests(None).remove(0);
        assert_eq!(
            request.context.batch.as_ref().unwrap().rate_limit,
            Some(100 * 1048576)
        );
        let receipt = receipt(&request, root.join("first.mp4"));
        let recorded = serde_json::to_value(&receipt.plan).unwrap();
        config.receipts.push(receipt);
        config.open = false;
        config.accept(
            vec![first],
            None,
            &root,
            prefs.default_download_rate_mib,
            false,
        );
        assert_eq!(config.rate_mib, 100.0);
        assert!(config.advanced_open);
        assert_eq!(prefs.default_download_rate_mib, 10.0);
        config.set_rate_override(5.0);
        assert_eq!(
            serde_json::to_value(&config.receipts[0].plan).unwrap(),
            recorded
        );
        config.accept(
            vec![prepared(2, &[80])],
            None,
            &root,
            prefs.default_download_rate_mib,
            false,
        );
        assert_eq!(config.rate_mib, 10.0);
        assert!(!config.advanced_open);
        assert_eq!(
            config.requests(None)[0]
                .context
                .batch
                .as_ref()
                .unwrap()
                .rate_limit,
            Some(10 * 1048576)
        );
    }

    fn frame(
        ctx: &egui::Context,
        config: &mut ConfigState,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ctx| {
                let _ = config.show(ctx, true, None);
            },
        )
    }
    fn texts(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn visit(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => out.push((
                    text.galley.job.text.clone(),
                    text.galley.rect.translate(text.pos.to_vec2()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        visit(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            visit(&shape.shape, &mut out);
        }
        out
    }
    fn click(ctx: &egui::Context, config: &mut ConfigState, size: egui::Vec2, label: &str) {
        let output = frame(ctx, config, size, vec![]);
        let pos = texts(&output)
            .into_iter()
            .find(|(text, _)| text == label)
            .unwrap_or_else(|| panic!("missing {label}"))
            .1
            .center();
        let _ = frame(
            ctx,
            config,
            size,
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
        let _ = frame(
            ctx,
            config,
            size,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
    }
    #[test]
    fn episode_choices_wrap_across_the_row_and_keep_original_part_selection() {
        fn visible_text(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
            fn find(shape: &egui::Shape, clip: egui::Rect, label: &str) -> Option<egui::Rect> {
                match shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        let rect = text.galley.rect.translate(text.pos.to_vec2());
                        clip.contains_rect(rect.shrink(0.5)).then_some(rect)
                    }
                    egui::Shape::Vec(shapes) => {
                        shapes.iter().find_map(|shape| find(shape, clip, label))
                    }
                    _ => None,
                }
            }
            output
                .shapes
                .iter()
                .find_map(|shape| find(&shape.shape, shape.clip_rect, label))
        }
        fn scroll_to(
            ctx: &egui::Context,
            config: &mut ConfigState,
            size: egui::Vec2,
            label: &str,
            delta: f32,
        ) -> egui::FullOutput {
            let at = dialog_bounds(egui::Rect::from_min_size(egui::Pos2::ZERO, size)).center();
            let mut output = frame(ctx, config, size, vec![]);
            for direction in [delta, -delta] {
                for _ in 0..12 {
                    if visible_text(&output, label).is_some() {
                        return output;
                    }
                    let _ = frame(
                        ctx,
                        config,
                        size,
                        vec![
                            egui::Event::PointerMoved(at),
                            egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                delta: egui::vec2(0.0, direction),
                                modifiers: Default::default(),
                            },
                        ],
                    );
                    // Let egui consume the wheel's animated offset before observing
                    // and clicking the newly visible widget in the next frame.
                    for _ in 0..24 {
                        output = frame(ctx, config, size, vec![]);
                    }
                }
            }
            panic!(
                "could not scroll to visible {label} at {size:?}; final text rectangles: {:?}",
                texts(&output)
                    .into_iter()
                    .filter(|(text, _)| text == label || text == "高级设置")
                    .collect::<Vec<_>>()
            );
        }
        let root = std::env::temp_dir().join("bili-config-episode-layout");
        let mut item = prepared(1, &[80]);
        let template = item.item.parts[0].clone();
        let template_catalog = item.catalogs[0].1.as_ref().unwrap().clone();
        item.item.parts = (1..=24)
            .map(|ordinal| {
                let mut part = template.clone();
                part.ordinal = ordinal;
                part.title = format!("Episode {ordinal}");
                part.reference.cid = Cid::new(1_000 + u64::from(ordinal)).unwrap();
                part
            })
            .collect();
        item.selected = item
            .item
            .parts
            .iter()
            .map(|part| part.reference.clone())
            .collect();
        item.catalogs = item
            .selected
            .iter()
            .map(|part| {
                let mut catalog = template_catalog.clone();
                catalog.part = part.clone();
                for representation in &mut catalog.representations {
                    representation.part = part.clone();
                }
                (part.clone(), Ok(catalog))
            })
            .collect();
        let original = item.selected.clone();
        let mut first_row_counts = Vec::new();
        for size in [egui::vec2(1180.0, 800.0), egui::vec2(760.0, 800.0)] {
            let ctx = egui::Context::default();
            crate::prefs::GuiPrefs::default().apply(&ctx);
            let mut config = ConfigState::default();
            config.accept(vec![item.clone()], None, &root, 0.0, false);
            let _ = frame(&ctx, &mut config, size, vec![]);
            let output = frame(&ctx, &mut config, size, vec![]);
            let episodes: Vec<_> = (1..=24)
                .filter_map(|ordinal| {
                    visible_text(&output, &format!("{ordinal}  Episode {ordinal}"))
                })
                .collect();
            assert!(
                episodes.len() >= 6,
                "the viewport must show multiple episode rows at {size:?}"
            );
            let columns = episodes
                .iter()
                .filter(|rect| (rect.top() - episodes[0].top()).abs() < 1.0)
                .count();
            assert!(
                columns >= 3,
                "episode choices still form a narrow list at {size:?}"
            );
            first_row_counts.push(columns);
            assert!(
                episodes
                    .iter()
                    .any(|rect| rect.right() > size.x * 0.5 + 100.0),
                "episode choices do not use the right side of the row"
            );
            assert!(
                episodes
                    .iter()
                    .all(|rect| rect.left() >= 0.0 && rect.right() <= size.x),
                "episode labels overflow the dialog viewport"
            );
            let bounds = dialog_bounds(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
            let output = scroll_to(&ctx, &mut config, size, "高级设置", -180.0);
            let advanced = visible_text(&output, "高级设置").unwrap();
            let footer = visible_text(&output, "开始下载 · 24 项").unwrap();
            assert!(
                bounds.contains_rect(advanced),
                "advanced options fell below the viewport after24episodes"
            );
            assert!(
                advanced.bottom() < footer.top() - 4.0,
                "advanced options are hidden behind the fixed footer"
            );
            let _ = scroll_to(&ctx, &mut config, size, "24  Episode 24", 180.0);
            click(&ctx, &mut config, size, "24  Episode 24");
            assert_eq!(config.rows[0].selected, original[..23]);
            let _ = scroll_to(&ctx, &mut config, size, "24  Episode 24", 180.0);
            click(&ctx, &mut config, size, "24  Episode 24");
            assert_eq!(config.rows[0].selected, original);
            config.admitted.push(original[0].clone());
            config.rows[0].selected.retain(|part| part != &original[0]);
            let _ = scroll_to(&ctx, &mut config, size, "1  Episode 1", 180.0);
            click(&ctx, &mut config, size, "1  Episode 1");
            assert!(
                !config.rows[0].selected.contains(&original[0]),
                "an admitted part became selectable"
            );
            assert_eq!(
                config
                    .requests(None)
                    .iter()
                    .map(|request| request.selection.part.clone())
                    .collect::<Vec<_>>(),
                original[1..]
            );
        }
        assert!(
            first_row_counts[0] > first_row_counts[1],
            "episode columns did not adapt to the available width"
        );
    }
    #[test]
    fn dialog_stays_centered_inside_safe_margins_and_advanced_is_inline_and_retained() {
        let root = std::env::temp_dir().join("bili-config-layout");
        for size in [
            egui::vec2(1180.0, 800.0),
            egui::vec2(900.0, 640.0),
            egui::vec2(320.0, 480.0),
        ] {
            let ctx = egui::Context::default();
            theme::apply_style(&ctx, false);
            let mut config = ConfigState::default();
            let item = prepared(1, &[80, 64]);
            config.accept(vec![item.clone()], None, &root, 10.0, false);
            let _ = frame(&ctx, &mut config, size, vec![]);
            let output = frame(&ctx, &mut config, size, vec![]);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            let bounds = ctx
                .memory(|memory| memory.area_rect(egui::Id::new("download-config")))
                .unwrap();
            assert!(
                screen.shrink(20.0).contains_rect(bounds),
                "{size:?}: {bounds:?}"
            );
            assert!(
                bounds.width() <= 720.1
                    && bounds.width() <= size.x * 0.8 + 1.0
                    && bounds.height() <= size.y * 0.8 + 1.0
            );
            assert!(
                (bounds.center() - screen.center()).length() < 1.0,
                "{size:?}: {bounds:?}"
            );
            let labels = texts(&output);
            {
                let label = "开始下载 · 1 项";
                let rect = labels
                    .iter()
                    .find(|(text, _)| text == label)
                    .unwrap_or_else(|| panic!("missing {label}"))
                    .1;
                assert!(bounds.expand(1.0).contains_rect(rect));
            }
            assert!(!labels.iter().any(|(text, _)| matches!(
                text.as_str(),
                "下载配置"
                    | "本次下载限速"
                    | "跟随当前账号"
                    | "重新检查画质"
                    | "添加成功后设为默认目录"
            )));
            if size.x >= 900.0 {
                for label in [
                    "保存目录（可直接编辑或选择文件夹）",
                    "字幕",
                    "弹幕",
                    "视频 1",
                    "1080P",
                ] {
                    let rect = labels
                        .iter()
                        .find(|(text, _)| text == label)
                        .unwrap_or_else(|| panic!("{size:?}: primary {label} must be visible"))
                        .1;
                    assert!(bounds.contains_rect(rect), "{size:?}: {label}: {rect:?}");
                }
            } else {
                // On tiny viewports the primary content scrolls normally. Reach the
                // final Advanced row with actual pointer wheel input before opening it.
                let _ = frame(
                    &ctx,
                    &mut config,
                    size,
                    vec![
                        egui::Event::PointerMoved(bounds.center()),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0.0, -500.0),
                            modifiers: Default::default(),
                        },
                    ],
                );
                for _ in 0..30 {
                    let _ = frame(&ctx, &mut config, size, vec![]);
                }
            }
            click(&ctx, &mut config, size, "高级设置");
            assert!(config.advanced_open);
            // egui 0.29 positions scroll contents before applying a queued target
            // offset, including a non-animated target. Paint the settled next frame.
            let _ = frame(&ctx, &mut config, size, vec![]);
            let output = frame(&ctx, &mut config, size, vec![]);
            let rate = texts(&output)
                .into_iter()
                .find(|(text, _)| text == "本次下载限速")
                .unwrap_or_else(|| panic!("{size:?}: expanded Advanced rate must be visible"))
                .1;
            assert!(bounds.contains_rect(rate), "{size:?}: {rate:?}");
            config.open = false;
            config.accept(vec![item], None, &root, 10.0, false);
            assert!(config.advanced_open);
        }
    }
}
