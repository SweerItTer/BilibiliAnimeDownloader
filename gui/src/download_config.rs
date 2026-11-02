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
    Highest,
    Exact(u32),
}
#[derive(Clone)]
struct Row {
    prepared: PreparedItem,
    selected: Vec<PartRef>,
    choice: QualityChoice,
    skip: bool,
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
                batch_id: format!("gui-{}", bili_core::JobId::new()),
                intent,
                ..Self::default()
            };
            self.rows = items
                .into_iter()
                .map(|prepared| Row {
                    selected: prepared.selected.clone(),
                    prepared,
                    choice: QualityChoice::Common,
                    skip: false,
                    fulfilled: false,
                })
                .collect();
        }
        self.catalog_profile = profile;
        self.error = None;
        self.verification = None;
        self.open = true;
    }
    pub fn exclude_admitted(&mut self, parts: Vec<PartRef>) {
        for row in &mut self.rows {
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
            QualityChoice::Highest => None,
            QualityChoice::Exact(id) => Some(id),
        }
    }
    fn catalog<'a>(row: &'a Row, part: &PartRef) -> Result<&'a QualityCatalog, String> {
        row.prepared
            .catalogs
            .iter()
            .find(|(p, _)| p == part)
            .ok_or_else(|| "画质尚未检查。".into())
            .and_then(|(_, r)| r.as_ref().map_err(backend::error_message))
    }
    fn row_error(&self, row: &Row) -> Option<String> {
        if row.skip || (row.fulfilled && row.selected.is_empty()) {
            return None;
        }
        if row.selected.is_empty() {
            return Some("请选择分集，或跳过此内容。".into());
        }
        let quality = self.quality_for(row);
        for part in &row.selected {
            let catalog = match Self::catalog(row, part) {
                Ok(c) => c,
                Err(e) => return Some(e),
            };
            let Some(source_part) = row
                .prepared
                .item
                .parts
                .iter()
                .find(|p| p.reference == *part)
            else {
                return Some("分集已变化，请重新选择。".into());
            };
            if catalog.select(&self.selection(row, source_part)).is_err() {
                let label =
                    quality.map_or_else(|| "可下载画质".into(), |id| quality_label(id, &self.rows));
                return Some(format!(
                    "{}没有可获取的 {label}；请选择其他画质或跳过。",
                    row.prepared
                        .item
                        .parts
                        .iter()
                        .find(|p| p.reference == *part)
                        .map_or_else(String::new, |p| format!("第 {} 项", p.ordinal))
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
        self.rows
            .iter()
            .filter(|r| !r.skip)
            .any(|r| !r.selected.is_empty())
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
        let mut open = self.open;
        let mut action = None;
        egui::Window::new("下载配置")
            .id(egui::Id::new("download-config"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(720.0)
            .max_width((ctx.screen_rect().width() - 40.0).max(280.0))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height((ctx.screen_rect().height() - 165.0).max(180.0))
                    .show(ui, |ui| {
                        if let Some(result) = &self.result {
                            ui.label(result);
                        }
                        if let Some(error) = &self.error {
                            ui.colored_label(theme::ERROR, error);
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
                            ui.colored_label(theme::ERROR, error);
                        }
                        ui.add_enabled_ui(available, |ui| {
                            ui.label("保存目录");
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
                                let width = (ui.available_width()
                                    - picker_width
                                    - ui.spacing().item_spacing.x)
                                    .max(0.0);
                                ui.add_sized(
                                    [width, height],
                                    theme::singleline_input(ui, &mut self.directory, "", height),
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
                            ui.checkbox(&mut self.make_default, "添加成功后设为默认目录");
                            ui.horizontal_wrapped(|ui| {
                                ui.label("统一画质");
                                let choices = qualities(&self.rows);
                                egui::ComboBox::from_id_salt("common-quality")
                                    .selected_text(self.quality.map_or_else(
                                        || "最高可用画质".into(),
                                        |q| quality_label(q, &self.rows),
                                    ))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.quality,
                                            None,
                                            "最高可用画质",
                                        );
                                        for (id, label) in choices {
                                            ui.selectable_value(&mut self.quality, Some(id), label);
                                        }
                                    });
                                ui.label("批次总限速");
                                ui.add(
                                    egui::DragValue::new(&mut self.rate_mib)
                                        .range(0.0..=10240.0)
                                        .speed(0.1)
                                        .suffix(" MiB/s"),
                                );
                                theme::muted(ui, "0 不限速");
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.checkbox(&mut self.subtitles, "字幕");
                                ui.checkbox(&mut self.danmaku, "弹幕");
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
                                    ui.selectable_value(
                                        &mut self.account,
                                        AccountChoice::Anonymous,
                                        "匿名",
                                    );
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
                            if self.account != previous {
                                action = Some(Operation::Prepare {
                                    refresh: true,
                                    inputs: self.inputs(),
                                    profile: self.effective_profile(auth),
                                });
                            }
                        });
                        if ui
                            .add_enabled(available, egui::Button::new("重新检查画质"))
                            .clicked()
                        {
                            action = Some(Operation::Prepare {
                                refresh: true,
                                inputs: self.inputs(),
                                profile: self.effective_profile(auth),
                            });
                        }
                        let profile = self.effective_profile(auth);
                        let profile_stale = profile != self.catalog_profile;
                        if profile_stale {
                            ui.colored_label(theme::ERROR, "账号已变化，请重新检查画质。");
                            if ui
                                .add_enabled(available, egui::Button::new("检查当前账号画质"))
                                .clicked()
                            {
                                action = Some(Operation::Prepare {
                                    refresh: true,
                                    inputs: self.inputs(),
                                    profile,
                                });
                            }
                        }
                        ui.separator();
                        let labels = qualities(&self.rows);
                        let common = self.quality;
                        for index in 0..self.rows.len() {
                            let error = self.row_error(&self.rows[index]);
                            let row = &mut self.rows[index];
                            let frame = egui::Frame::group(ui.style()).stroke(egui::Stroke::new(
                                1.0_f32,
                                if error.is_some() {
                                    theme::ERROR
                                } else {
                                    theme::border()
                                },
                            ));
                            frame.show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(&row.prepared.item.title).strong());
                                    ui.add_enabled_ui(available, |ui| {
                                        ui.checkbox(&mut row.skip, "跳过");
                                    });
                                });
                                ui.add_enabled_ui(available && !row.skip, |ui| {
                                    egui::ComboBox::from_id_salt(("row-quality", index))
                                        .selected_text(match row.choice {
                                            QualityChoice::Common => format!(
                                                "统一设置 · {}",
                                                common.map_or_else(
                                                    || "最高可用".into(),
                                                    |q| labels
                                                        .iter()
                                                        .find(|(id, _)| *id == q)
                                                        .map_or_else(
                                                            || format!("画质 {q}"),
                                                            |(_, l)| l.clone()
                                                        )
                                                )
                                            ),
                                            QualityChoice::Highest => "本项最高可用".into(),
                                            QualityChoice::Exact(q) => {
                                                labels.iter().find(|(id, _)| *id == q).map_or_else(
                                                    || format!("画质 {q}"),
                                                    |(_, l)| l.clone(),
                                                )
                                            }
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut row.choice,
                                                QualityChoice::Common,
                                                "跟随统一设置",
                                            );
                                            ui.selectable_value(
                                                &mut row.choice,
                                                QualityChoice::Highest,
                                                "本项最高可用",
                                            );
                                            for (id, label) in &labels {
                                                ui.selectable_value(
                                                    &mut row.choice,
                                                    QualityChoice::Exact(*id),
                                                    label,
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
                                    for part in &row.prepared.item.parts {
                                        let mut selected = row.selected.contains(&part.reference);
                                        if ui
                                            .add_enabled_ui(
                                                !self.admitted.contains(&part.reference),
                                                |ui| {
                                                    ui.checkbox(
                                                        &mut selected,
                                                        format!("{}  {}", part.ordinal, part.title),
                                                    )
                                                },
                                            )
                                            .inner
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
                                if let Some(error) = error {
                                    ui.colored_label(theme::ERROR, error);
                                }
                            });
                            ui.add_space(6.0);
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
                let count = self
                    .rows
                    .iter()
                    .filter(|r| !r.skip)
                    .map(|r| r.selected.len())
                    .sum::<usize>();
                let blocked = !self.can_submit(auth);
                ui.horizontal(|ui| {
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
                    if ui
                        .button(if self.receipts.is_empty() {
                            "关闭"
                        } else {
                            "继续添加其他内容"
                        })
                        .clicked()
                    {
                        self.open = false;
                    }
                });
            });
        self.open &= open;
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.open = false;
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
            .filter(|r| !r.skip)
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
fn qualities(rows: &[Row]) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for row in rows {
        for (_, result) in &row.prepared.catalogs {
            if let Ok(catalog) = result {
                for entry in &catalog.advertised {
                    if !out.iter().any(|(id, _)| *id == entry.id) {
                        out.push((entry.id, entry.label.clone()));
                    }
                }
            }
        }
    }
    out.sort_by_key(|(id, _)| std::cmp::Reverse(*id));
    out
}
fn quality_label(id: u32, rows: &[Row]) -> String {
    qualities(rows)
        .into_iter()
        .find(|(q, _)| *q == id)
        .map_or_else(|| format!("画质 {id}"), |(_, l)| l)
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
                    label: format!("画质 {q}"),
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
                    width: Some(1280),
                    height: Some(720),
                    fps: None,
                    dynamic_range: None,
                    color: None,
                    bandwidth: 1000,
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
            false,
        );
        config.quality = Some(112);
        assert!(config.row_error(&config.rows[0]).is_some());
        assert!(config.row_error(&config.rows[1]).is_none());
        assert!(!config.can_submit(None));
        config.rows[0].choice = QualityChoice::Highest;
        assert!(config.can_submit(None));
        let requests = config.requests(None);
        assert_eq!(requests[0].selection.quality, None);
        assert_eq!(requests[1].selection.quality, Some(112));
        assert_eq!(requests[0].context.project.as_ref().unwrap().ordinal, 3);
        config.rows[0].choice = QualityChoice::Exact(80);
        assert!(!config.can_submit(None));
        config.rows[0].skip = true;
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
        config.accept(vec![item], None, &root, false);
        config.quality = Some(112);
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
    fn acknowledged_receipts_remain_immutable_and_manual_empty_selection_is_not_fulfilled() {
        let root = std::env::temp_dir().join("bili-receipt-tests");
        let mut config = ConfigState::default();
        let items = vec![prepared(1, &[64]), prepared(2, &[64])];
        config.accept(items.clone(), None, &root, false);
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
        config.quality = Some(112);
        assert_eq!(
            serde_json::to_value(&config.receipts[0].plan).unwrap(),
            original
        );
        config.open = false;
        config.accept(items, None, &root, false);
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
        config.accept(vec![first.clone()], None, &root, false);
        config.directory = std::env::temp_dir()
            .join("deliberate-draft")
            .display()
            .to_string();
        config.rate_mib = 2.0;
        config.accept(vec![first.clone()], None, &root, true);
        assert_eq!(config.rate_mib, 2.0);
        let mut changed = first;
        changed.selected = vec![second.reference.clone()];
        assert!(!config.matches_intent(std::slice::from_ref(&changed)));
        config.accept(vec![changed], None, &root, false);
        assert_eq!(config.rows[0].selected, vec![second.reference]);
        assert_eq!(config.directory, root.display().to_string());
        assert_eq!(config.rate_mib, 0.0);
    }
}
