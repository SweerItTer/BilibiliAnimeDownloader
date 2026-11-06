//! Browsing drafts, submitted queries and bulk selection are independent.
use crate::{
    backend::{Bridge, Operation, PrepareInput},
    covers::CoverCache,
    theme,
};
use bili_core::{
    BrowseConditions, BrowseQuery, Candidate, CandidatePage, ContentRef, PgcSection,
    SearchCategory, SearchFilter, SearchQuery, SearchSort,
};
use egui::Ui;
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Empty,
    Keyword,
    Content,
}
/// Chooses only the UI route. Core validates identifiers and URLs.
pub fn classify_input(input: &str) -> InputKind {
    let input = input.trim();
    if input.is_empty() {
        return InputKind::Empty;
    }
    let lower = input.to_ascii_lowercase();
    let numbered = ["av", "ss", "ep"].iter().any(|prefix| {
        lower
            .strip_prefix(*prefix)
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
    });
    if lower.contains("://")
        || lower.starts_with("www.bilibili.com/")
        || lower.starts_with("b23.tv/")
        || numbered
        || (input.starts_with("BV") && !input.contains(char::is_whitespace))
    {
        InputKind::Content
    } else {
        InputKind::Keyword
    }
}
pub fn reference_input(reference: &ContentRef) -> String {
    match reference {
        ContentRef::Video(id) => format!("av{id}"),
        ContentRef::Bangumi(id) => format!("ss{id}"),
        ContentRef::Episode(id) => format!("ep{id}"),
    }
}
pub fn category_name(category: SearchCategory) -> &'static str {
    match category {
        SearchCategory::All => "全部类型",
        SearchCategory::Video => "视频",
        SearchCategory::Bangumi => "番剧",
        SearchCategory::Film => "影视",
    }
}
fn sort_name(sort: SearchSort) -> &'static str {
    match sort {
        SearchSort::Relevance => "综合排序",
        SearchSort::Views => "最多播放",
        SearchSort::Newest => "最新发布",
        SearchSort::Danmaku => "最多弹幕",
        SearchSort::Favorites => "最多收藏",
        SearchSort::Comments => "最多评论",
    }
}
pub fn duration(ms: Option<u64>) -> String {
    ms.map_or_else(
        || "时长未知".into(),
        |ms| {
            let s = ms / 1000;
            if s >= 3600 {
                format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
            } else {
                format!("{}:{:02}", s / 60, s % 60)
            }
        },
    )
}

pub struct SearchState {
    pub input: String,
    pub category: SearchCategory,
    sort: SearchSort,
    pub results: Option<CandidatePage>,
    submitted: Option<SearchQuery>,
    requested: Option<SearchQuery>,
    pub request_focus: bool,
    pub reset_scroll: bool,
    pub stale: bool,
    started: Option<Instant>,
    pub bulk: bool,
    pub selected: Vec<Candidate>,
    page_edit: Option<String>,
    page_error: Option<String>,
    filter: SearchFilter,
    browse: Option<BrowseQuery>,
    requested_browse: Option<BrowseQuery>,
    conditions: Option<BrowseConditions>,
    condition_cache: std::collections::HashMap<PgcSection, BrowseConditions>,
    browse_history: std::collections::HashMap<PgcSection, BrowseQuery>,
    filter_notice: Option<String>,
    pub home_requested: bool,
}
impl Default for SearchState {
    fn default() -> Self {
        Self {
            input: String::new(),
            category: SearchCategory::Video,
            sort: SearchSort::Relevance,
            results: None,
            submitted: None,
            requested: None,
            request_focus: false,
            reset_scroll: false,
            stale: false,
            started: None,
            bulk: false,
            selected: Vec::new(),
            page_edit: None,
            page_error: None,
            filter: SearchFilter::default(),
            browse: None,
            requested_browse: None,
            conditions: None,
            condition_cache: Default::default(),
            browse_history: Default::default(),
            filter_notice: None,
            home_requested: false,
        }
    }
}
impl SearchState {
    pub fn accept_results(&mut self, results: CandidatePage) {
        self.reset_scroll = true;
        self.browse = self.requested_browse.take();
        if let Some(query) = &self.browse {
            self.browse_history.insert(query.section, query.clone());
        }
        self.results = Some(results);
        self.submitted = self.requested.take();
        self.started = None;
        self.stale = false;
        self.page_edit = None;
        self.page_error = None;
    }
    pub fn discard_request(&mut self) {
        self.requested = None;
        self.requested_browse = None;
        self.started = None;
        if let Some(q) = &self.submitted {
            self.category = q.category;
            self.sort = q.sort;
            self.filter = q.filter.clone();
        } else if let Some(query) = &self.browse {
            self.category = query.section.category();
            self.conditions = self.condition_cache.get(&query.section).cloned();
        }
    }
    pub fn requested_query(&self) -> Option<&SearchQuery> {
        self.requested.as_ref()
    }
    pub fn set_requested(&mut self, query: SearchQuery) {
        self.category = query.category;
        self.sort = query.sort;
        self.filter = query.filter.clone();
        self.requested = Some(query);
        self.requested_browse = None;
    }
    pub fn set_requested_browse(&mut self, query: BrowseQuery) {
        self.category = query.section.category();
        self.conditions = self.condition_cache.get(&query.section).cloned();
        self.requested_browse = Some(query);
        self.requested = None;
    }
    pub fn start_wait(&mut self) {
        self.started = Some(Instant::now());
    }
    pub fn pending(&self) -> bool {
        self.started.is_some()
    }
    pub fn mark_stale(&mut self) {
        self.stale = self.results.is_some();
    }
    pub fn choose_category(&mut self, category: SearchCategory) -> Option<Operation> {
        self.home_requested = true;
        if category == self.category {
            // A failed first conditions request has no committed category
            // context. An explicit click can retry it, while a loaded active
            // tab and an operation still pending do not refresh implicitly.
            let loaded = self
                .conditions
                .as_ref()
                .is_some_and(|c| c.section.category() == category)
                || self
                    .results
                    .as_ref()
                    .is_some_and(|r| r.pages.iter().any(|p| p.category == category));
            if category != SearchCategory::Video && !loaded && !self.pending() {
                return Some(Operation::BrowseConditions(
                    if category == SearchCategory::Bangumi {
                        PgcSection::Anime
                    } else {
                        PgcSection::Film
                    },
                ));
            }
            return None;
        }
        self.category = category;
        self.filter = SearchFilter::default();
        if !SearchSort::available_for(category).contains(&self.sort) {
            self.sort = SearchSort::Relevance;
        }
        if category == SearchCategory::Video {
            let action = self.submit_search();
            if action.is_none() {
                self.results = None;
                self.submitted = None;
                self.browse = None;
                self.requested_browse = None;
                self.conditions = None;
                self.page_edit = None;
            }
            action
        } else {
            Some(Operation::BrowseConditions(
                if category == SearchCategory::Bangumi {
                    PgcSection::Anime
                } else {
                    PgcSection::Film
                },
            ))
        }
    }
    fn submit_search(&mut self) -> Option<Operation> {
        let query = SearchQuery {
            keyword: self.input.trim().to_owned(),
            category: self.category,
            page: 1,
            sort: self.sort,
            filter: self.filter.clone(),
        };
        if classify_input(&self.input) != InputKind::Keyword || query.validate().is_err() {
            return None;
        }
        self.requested = Some(query.clone());
        self.requested_browse = None;
        Some(Operation::Search(query))
    }
    pub fn accept_conditions(&mut self, conditions: BrowseConditions) -> Operation {
        let previous = self
            .requested_browse
            .clone()
            .or_else(|| self.browse.clone());
        let mut query = self
            .browse_history
            .get(&conditions.section)
            .cloned()
            .unwrap_or_else(|| BrowseQuery::new(conditions.section));
        query.page = 1;
        let mut removed = Vec::new();
        if let Some(previous) = previous {
            for group in &conditions.filters {
                if self
                    .conditions
                    .as_ref()
                    .is_some_and(|old| old.filters.iter().any(|choice| choice.field == group.field))
                {
                    let value = previous
                        .filters
                        .get(&group.field)
                        .map_or("-1", String::as_str);
                    if group.choices.iter().any(|choice| choice.value == value) {
                        if value == "-1" {
                            query.filters.remove(&group.field);
                        } else {
                            query.filters.insert(group.field, value.to_owned());
                        }
                    } else {
                        query.filters.remove(&group.field);
                        if value != "-1" {
                            removed.push(group.label.clone());
                        }
                    }
                }
            }
            for field in previous.filters.keys() {
                if !conditions.filters.iter().any(|group| group.field == *field) {
                    if let Some(label) = self
                        .conditions
                        .as_ref()
                        .and_then(|old| old.filters.iter().find(|group| group.field == *field))
                        .map(|group| group.label.clone())
                    {
                        removed.push(label);
                    }
                }
            }
            query.order = previous.order;
            query.sort = previous.sort;
        }
        query.filters.retain(|field, value| {
            conditions.filters.iter().any(|group| {
                group.field == *field && group.choices.iter().any(|choice| choice.value == *value)
            })
        });
        if !conditions
            .orders
            .iter()
            .any(|choice| Some(choice.id) == query.order && choice.sorts.contains(&query.sort))
        {
            query.order = conditions.orders.first().map(|choice| choice.id);
            query.sort = conditions
                .orders
                .first()
                .and_then(|choice| choice.sorts.first())
                .copied()
                .unwrap_or(0);
        }
        self.filter_notice = (!removed.is_empty())
            .then(|| format!("当前分区不支持这些筛选，已移除：{}", removed.join("、")));
        self.condition_cache
            .insert(conditions.section, conditions.clone());
        self.conditions = Some(conditions);
        self.requested = None;
        self.requested_browse = Some(query.clone());
        Operation::Browse(query)
    }
    pub fn result_category(&self, reference: &ContentRef) -> Option<SearchCategory> {
        self.results
            .as_ref()?
            .pages
            .iter()
            .flat_map(|p| &p.items)
            .find(|c| &c.reference == reference)
            .map(|c| c.category)
    }
    fn choose_sort(&mut self, sort: SearchSort) -> Option<Operation> {
        if sort == self.sort || !SearchSort::available_for(self.category).contains(&sort) {
            return None;
        }
        self.sort = sort;
        self.submit_search()
    }
    pub fn header(&mut self, ui: &mut Ui, available: bool) -> Option<Operation> {
        let mut action = None;
        readable_frame().show(ui, |ui| {
            ui.add_enabled_ui(available, |ui| {
                ui.horizontal(|ui| {
                    for category in [
                        SearchCategory::Video,
                        SearchCategory::Bangumi,
                        SearchCategory::Film,
                    ] {
                        if ui
                            .selectable_label(self.category == category, category_name(category))
                            .clicked()
                        {
                            action = self.choose_category(category);
                        }
                    }
                });
            });
        });
        action
    }
    pub fn input(&mut self, ui: &mut Ui, available: bool, width: f32) -> Option<Operation> {
        let mut submit = false;
        ui.horizontal(|ui| {
            let response = ui.add_sized(
                [width.max(90.0), 34.0],
                theme::singleline_input(ui, &mut self.input, "搜索，或粘贴链接", 34.0),
            );
            if self.request_focus {
                response.request_focus();
                self.request_focus = false;
            }
            submit = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            submit |= ui
                .add_enabled(
                    available && classify_input(&self.input) != InputKind::Empty,
                    egui::Button::new("搜索"),
                )
                .clicked();
        });
        if submit && available {
            match classify_input(&self.input) {
                InputKind::Content => Some(Operation::Resolve(self.input.trim().to_owned())),
                InputKind::Keyword => self.submit_search(),
                InputKind::Empty => None,
            }
        } else {
            None
        }
    }
    pub fn show(
        &mut self,
        ui: &mut Ui,
        available: bool,
        _reduced: bool,
        covers: &mut CoverCache,
        bridge: Option<&Bridge>,
    ) -> Option<Operation> {
        let mut action = None;
        // Keep text legible over user images without tinting the image itself.
        readable_frame().show(ui, |ui| {
            ui.add_enabled_ui(available, |ui| {
                if self.category == SearchCategory::Video {
                    ui.horizontal_wrapped(|ui| {
                        let mut channel = self.filter.video_channel;
                        if ui.selectable_label(channel.is_none(), "全部分区").clicked() {
                            channel = None;
                        }
                        for &(id, label) in SearchFilter::video_channels() {
                            if ui.selectable_label(channel == Some(id), label).clicked() {
                                channel = Some(id);
                            }
                        }
                        if channel != self.filter.video_channel {
                            self.filter.video_channel = channel;
                            action = self.submit_search();
                        }
                    });
                } else {
                    let section = self
                        .requested_browse
                        .as_ref()
                        .or(self.browse.as_ref())
                        .map(|q| q.section)
                        .or_else(|| self.conditions.as_ref().map(|c| c.section));
                    ui.horizontal_wrapped(|ui| {
                        for &(choice, label) in if self.category == SearchCategory::Bangumi {
                            &[
                                (PgcSection::Anime, "番剧"),
                                (PgcSection::ChineseAnimation, "国创"),
                            ][..]
                        } else {
                            &[
                                (PgcSection::Film, "电影"),
                                (PgcSection::Tv, "电视剧"),
                                (PgcSection::Documentary, "纪录片"),
                                (PgcSection::Variety, "综艺"),
                            ][..]
                        } {
                            if ui
                                .selectable_label(section == Some(choice), label)
                                .clicked()
                            {
                                action = Some(Operation::BrowseConditions(choice));
                            }
                        }
                    });
                    if let Some(conditions) = &self.conditions {
                        if conditions.section.category() == self.category {
                            let mut query = self
                                .requested_browse
                                .as_ref()
                                .or(self.browse.as_ref())
                                .filter(|query| query.section == conditions.section)
                                .cloned()
                                .unwrap_or_else(|| BrowseQuery::new(conditions.section));
                            let mut changed = false;
                            ui.horizontal_wrapped(|ui| {
                                for group in &conditions.filters {
                                    ui.horizontal(|ui| {
                                        ui.label(&group.label);
                                        let mut value = query
                                            .filters
                                            .get(&group.field)
                                            .cloned()
                                            .unwrap_or_else(|| "-1".into());
                                        let label = group
                                            .choices
                                            .iter()
                                            .find(|choice| choice.value == value)
                                            .map_or("全部", |choice| choice.label.as_str());
                                        egui::ComboBox::from_id_salt(("pgc-filter", group.field))
                                            .selected_text(label)
                                            .width(132.0)
                                            .height(320.0)
                                            .show_ui(ui, |ui| {
                                                for choice in &group.choices {
                                                    if ui
                                                        .selectable_value(
                                                            &mut value,
                                                            choice.value.clone(),
                                                            &choice.label,
                                                        )
                                                        .changed()
                                                    {
                                                        changed = true;
                                                    }
                                                }
                                            });
                                        if value == "-1" {
                                            query.filters.remove(&group.field);
                                        } else {
                                            query.filters.insert(group.field, value);
                                        }
                                    });
                                }
                            });
                            ui.horizontal_wrapped(|ui| {
                                for order in &conditions.orders {
                                    if ui
                                        .selectable_label(
                                            query.order == Some(order.id),
                                            &order.label,
                                        )
                                        .clicked()
                                    {
                                        query.order = Some(order.id);
                                        if !order.sorts.contains(&query.sort) {
                                            query.sort = order.sorts[0];
                                        }
                                        changed = true;
                                    }
                                }
                                if conditions.orders.iter().any(|order| {
                                    Some(order.id) == query.order && order.sorts.len() > 1
                                }) && ui
                                    .button(if query.sort == 0 { "降序" } else { "升序" })
                                    .clicked()
                                {
                                    query.sort = 1 - query.sort;
                                    changed = true;
                                }
                            });
                            if changed {
                                query.page = 1;
                                self.filter_notice = None;
                                self.requested = None;
                                self.requested_browse = Some(query.clone());
                                action = Some(Operation::Browse(query));
                            }
                            if let Some(notice) = &self.filter_notice {
                                theme::muted(ui, notice);
                            }
                        }
                    }
                }
            });

            ui.horizontal_wrapped(|ui| {
                if self.category == SearchCategory::Video {
                    for &sort in SearchSort::available_for(self.category) {
                        if ui
                            .add_enabled(
                                available,
                                egui::Button::new(sort_name(sort))
                                    .selected(self.sort == sort)
                                    .rounding(6.0),
                            )
                            .clicked()
                        {
                            action = self.choose_sort(sort);
                        }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            available,
                            egui::Button::new(if self.bulk {
                                "退出多选"
                            } else {
                                "批量下载"
                            }),
                        )
                        .clicked()
                    {
                        self.bulk = !self.bulk;
                        if !self.bulk {
                            self.selected.clear();
                        }
                    }
                    if self.bulk
                        && ui
                            .add_enabled(
                                available && !self.selected.is_empty(),
                                egui::Button::new(format!("配置下载 · {}", self.selected.len())),
                            )
                            .clicked()
                    {
                        action = Some(Operation::Prepare {
                            refresh: false,
                            inputs: self
                                .selected
                                .iter()
                                .map(|c| PrepareInput {
                                    input: reference_input(&c.reference),
                                    parts: None,
                                    category: c.category,
                                })
                                .collect(),
                            profile: None,
                        });
                    }
                });
            });
        });
        ui.add_space(12.0);
        if self.stale {
            readable_frame().show(ui, |ui| {
                ui.colored_label(theme::error_color(), "本次搜索未更新，以下保留上次结果。");
            });
        }
        let items: Vec<&Candidate> = self
            .results
            .as_ref()
            .map(|r| r.pages.iter().flat_map(|p| p.items.iter()).collect())
            .unwrap_or_default();
        if self.results.is_none() && !self.pending() {
            // ScrollArea offers infinite content height; center within its
            // visible viewport instead of extending the scrollable document.
            let height = (ui.clip_rect().bottom() - ui.cursor().top()).max(80.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), height),
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    ui.label(
                        egui::RichText::new("搜索以获取结果")
                            .size(24.0)
                            .color(theme::muted_color()),
                    );
                },
            );
        } else if self.results.is_some() && items.is_empty() && !self.pending() {
            readable_frame().show(ui, |ui| {
                theme::muted(ui, "没有找到相关内容。");
            });
        }
        let columns = ((ui.available_width() + 16.0) / 240.0)
            .floor()
            .clamp(1.0, 5.0) as usize;
        for chunk in items.chunks(columns) {
            ui.columns(columns, |uis| {
                for (column, item) in uis.iter_mut().zip(chunk) {
                    column.push_id(reference_input(&item.reference), |ui| {
                        let selected = self.selected.iter().any(|c| c.reference == item.reference);
                        let width = ui.available_width();
                        let size = crate::covers::cover_size(item.category, width);
                        let response = ui
                            .allocate_ui_with_layout(
                                size,
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    if !covers.show(ui, bridge, item.cover.as_deref(), size, 4.0) {
                                        let (rect, _) =
                                            ui.allocate_exact_size(size, egui::Sense::hover());
                                        ui.painter().rect_filled(rect, 4.0, theme::soft());
                                        theme::icon(
                                            ui.painter(),
                                            egui::Rect::from_center_size(
                                                rect.center(),
                                                egui::vec2(24.0, 24.0),
                                            ),
                                            theme::Icon::Play,
                                            theme::primary(),
                                        );
                                    }
                                },
                            )
                            .response
                            .interact(egui::Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        readable_frame().show(ui, |ui| {
                            ui.set_min_width((width - 16.0).max(0.0));
                            let title = ui.add(
                                egui::Label::new(&item.title)
                                    .wrap()
                                    .sense(egui::Sense::click()),
                            );
                            if let Some(author) = &item.author {
                                theme::muted(ui, author);
                            }
                            let stats = [
                                item.plays.map(|n| format!("播放 {}", compact_count(n))),
                                item.danmaku.map(|n| format!("弹幕 {}", compact_count(n))),
                                item.published_at.map(published),
                            ]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(" · ");
                            if !stats.is_empty() {
                                theme::muted(ui, stats);
                            }
                            if self.bulk {
                                let mut check = selected;
                                let changed = ui
                                    .add_enabled(
                                        available,
                                        egui::Checkbox::new(&mut check, "选择此内容"),
                                    )
                                    .changed();
                                if available && (response.clicked() || title.clicked() || changed) {
                                    if selected {
                                        self.selected.retain(|c| c.reference != item.reference);
                                    } else {
                                        self.selected.push((**item).clone());
                                    }
                                }
                            } else if available && (response.clicked() || title.clicked()) {
                                action = Some(Operation::Resolve(reference_input(&item.reference)));
                            }
                            theme::muted(
                                ui,
                                format!(
                                    "{} · {}",
                                    category_name(item.category),
                                    duration(item.duration_ms)
                                ),
                            );
                        });
                        ui.add_space(18.0);
                    });
                }
            });
        }
        if let Some(current) = self
            .browse
            .as_ref()
            .map(|q| q.page)
            .or_else(|| self.submitted.as_ref().map(|q| q.page))
        {
            let page = self.results.as_ref().and_then(|r| r.pages.first());
            let total_pages = page.and_then(|p| {
                p.total_pages.or_else(|| {
                    p.total
                        .zip(p.page_size)
                        .filter(|(_, size)| *size > 0)
                        .map(|(total, size)| total.div_ceil(size as u64).clamp(1, 1000) as u32)
                })
            });
            let can_next = page.is_some_and(|p| p.has_more) && current < 1000;
            readable_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(available && current > 1, egui::Button::new("<"))
                        .clicked()
                    {
                        action = self.page(current - 1);
                    }
                    let mut navigate = None;
                    if let Some(draft) = &mut self.page_edit {
                        let id = ui.make_persistent_id("page-number");
                        let response = ui
                            .add_enabled_ui(available, |ui| {
                                ui.add_sized(
                                    [48.0, 30.0],
                                    theme::singleline_input(ui, draft, "", 30.0).id(id),
                                )
                            })
                            .inner;
                        if available
                            && response.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            navigate = Some(draft.clone());
                        }
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            self.page_edit = None;
                            self.page_error = None;
                        }
                    } else if ui
                        .add_enabled(
                            available,
                            egui::Button::new(current.to_string()).frame(false),
                        )
                        .on_hover_text("点击修改页码")
                        .clicked()
                    {
                        self.page_edit = Some(current.to_string());
                        ui.memory_mut(|m| m.request_focus(ui.make_persistent_id("page-number")));
                    }
                    ui.label(format!(
                        "/ {}",
                        total_pages.map_or_else(|| "?".into(), |n| n.to_string())
                    ));
                    if ui
                        .add_enabled(available && can_next, egui::Button::new(">"))
                        .clicked()
                    {
                        action = self.page(current + 1);
                    }
                    if let Some(draft) = navigate {
                        match draft.parse::<u32>() {
                            Ok(n) if n > 0 && n <= total_pages.unwrap_or(1000) => {
                                self.page_error = None;
                                self.page_edit = None;
                                action = self.page(n);
                            }
                            _ => {
                                self.page_error = Some(format!(
                                    "请输入 1–{} 之间的整数页码。",
                                    total_pages.unwrap_or(1000)
                                ))
                            }
                        }
                    }
                    if let Some(error) = &self.page_error {
                        ui.colored_label(theme::error_color(), error);
                    }
                });
            });
        }
        action
    }
    fn page(&mut self, page: u32) -> Option<Operation> {
        if let Some(mut query) = self.browse.clone() {
            query.page = page;
            self.requested_browse = Some(query.clone());
            return Some(Operation::Browse(query));
        }
        let mut query = self.submitted.clone()?;
        query.page = page;
        self.requested = Some(query.clone());
        Some(Operation::Search(query))
    }
}
fn readable_frame() -> egui::Frame {
    egui::Frame::none()
        .rounding(6.0)
        .fill(theme::card())
        .inner_margin(egui::Margin::symmetric(8.0, 6.0))
}
fn compact_count(n: u64) -> String {
    if n >= 10000 {
        format!("{:.1}万", n as f64 / 10000.0)
    } else {
        n.to_string()
    }
}
fn published(epoch: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if epoch > now {
        "发布时间未知".into()
    } else {
        let days = (now - epoch) / 86400;
        if days == 0 {
            "今天".into()
        } else {
            format!("{days} 天前")
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn render(
        ctx: &egui::Context,
        state: &mut SearchState,
        covers: &mut CoverCache,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<Operation>) {
        render_available(ctx, state, covers, events, true)
    }

    fn render_available(
        ctx: &egui::Context,
        state: &mut SearchState,
        covers: &mut CoverCache,
        events: Vec<egui::Event>,
        available: bool,
    ) -> (egui::FullOutput, Option<Operation>) {
        let mut action = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(760.0, 640.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                ctx.layer_painter(egui::LayerId::background()).rect_filled(
                    ctx.screen_rect(),
                    0.0,
                    egui::Color32::from_rgb(16, 20, 30),
                );
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        state.header(ui, available);
                        ui.add_space(6.0);
                        action = state.show(ui, available, true, covers, None);
                    });
            },
        );
        (output, action)
    }

    fn click_label(
        ctx: &egui::Context,
        state: &mut SearchState,
        covers: &mut CoverCache,
        label: &str,
        occurrence: usize,
        available: bool,
    ) -> Option<Operation> {
        let (output, _) = render_available(ctx, state, covers, vec![], available);
        let pos = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.galley.rect.translate(text.pos.to_vec2()).center())
                }
                _ => None,
            })
            .nth(occurrence)
            .unwrap_or_else(|| panic!("missing label {label} occurrence {occurrence}"));
        render_available(
            ctx,
            state,
            covers,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            available,
        );
        render_available(
            ctx,
            state,
            covers,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            available,
        )
        .1
    }

    fn page_results(page: u32, items: Vec<Candidate>) -> CandidatePage {
        CandidatePage {
            pages: vec![bili_core::CategoryPage {
                category: SearchCategory::Video,
                page,
                total: Some(60),
                has_more: page < 3,
                page_size: Some(20),
                total_pages: Some(3),
                items,
            }],
        }
    }

    fn page_state(items: Vec<Candidate>) -> SearchState {
        let mut state = SearchState {
            input: "fixture".into(),
            ..Default::default()
        };
        state.submit_search();
        state.accept_results(page_results(1, items));
        state
    }

    fn replace_and_enter(text: &str) -> Vec<egui::Event> {
        vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            },
            egui::Event::Text(text.into()),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn pending_page_editor_cannot_replace_the_inflight_query() {
        let ctx = egui::Context::default();
        theme::apply_style(&ctx, false);
        let mut covers = CoverCache::default();
        let mut state = page_state(vec![]);
        click_label(&ctx, &mut state, &mut covers, "1", 0, true);
        click_label(&ctx, &mut state, &mut covers, "1", 0, true);
        assert!(state.page_edit.is_some());
        let Some(Operation::Search(query)) = state.page(2) else {
            panic!("missing page request")
        };
        assert_eq!(query.page, 2);
        state.start_wait();

        let (_, action) =
            render_available(&ctx, &mut state, &mut covers, replace_and_enter("3"), false);
        assert!(action.is_none());
        assert_eq!(state.page_edit.as_deref(), Some("1"));
        assert_eq!(state.requested_query().unwrap().page, 2);
        state.accept_results(page_results(2, vec![]));
        assert_eq!(state.submitted.as_ref().unwrap().page, 2);
        assert_eq!(state.results.as_ref().unwrap().pages[0].page, 2);
    }

    #[test]
    fn submitting_inline_page_closes_the_editor_before_waiting() {
        let ctx = egui::Context::default();
        theme::apply_style(&ctx, false);
        let mut covers = CoverCache::default();
        let mut state = page_state(vec![]);
        click_label(&ctx, &mut state, &mut covers, "1", 0, true);
        click_label(&ctx, &mut state, &mut covers, "1", 0, true);
        let (_, action) =
            render_available(&ctx, &mut state, &mut covers, replace_and_enter("2"), true);
        assert!(matches!(action, Some(Operation::Search(query)) if query.page == 2));
        assert!(state.page_edit.is_none());
        assert_eq!(state.requested_query().unwrap().page, 2);
    }

    #[test]
    fn pending_preparation_keeps_its_bulk_selection_and_mode() {
        let ctx = egui::Context::default();
        theme::apply_style(&ctx, false);
        let mut covers = CoverCache::default();
        let candidate = |id, title: &str| Candidate {
            reference: ContentRef::Video(bili_core::Aid::new(id).unwrap()),
            category: SearchCategory::Video,
            title: title.into(),
            cover: None,
            duration_ms: None,
            author: None,
            published_at: None,
            plays: None,
            danmaku: None,
        };
        let first = candidate(41, "First");
        let second = candidate(42, "Second");
        let mut state = page_state(vec![first.clone(), second]);
        state.bulk = true;
        state.selected.push(first.clone());
        let action = click_label(&ctx, &mut state, &mut covers, "配置下载 · 1", 0, true);
        assert!(matches!(action,
            Some(Operation::Prepare { inputs, .. })
                if inputs.len() == 1 && inputs[0].input == "av41"));
        state.start_wait();

        for (label, occurrence) in [
            ("退出多选", 0),
            ("Second", 0),
            ("选择此内容", 1),
            ("配置下载 · 1", 0),
        ] {
            assert!(
                click_label(&ctx, &mut state, &mut covers, label, occurrence, false,).is_none()
            );
            assert!(state.bulk);
            assert_eq!(state.selected.len(), 1);
            assert_eq!(state.selected[0].reference, first.reference);
        }
    }

    #[test]
    fn local_backing_preserves_legible_search_text_and_cover_navigation() {
        let ctx = egui::Context::default();
        theme::apply_style(&ctx, false);
        let mut covers = CoverCache::default();
        covers.insert(
            &ctx,
            "fixture:cover".into(),
            Some(egui::ColorImage::new([160, 90], egui::Color32::BLUE)),
        );
        let texture = covers.texture("fixture:cover").unwrap();
        let mut state = SearchState {
            input: "fixture".into(),
            ..Default::default()
        };
        state.submit_search();
        state.accept_results(CandidatePage {
            pages: vec![bili_core::CategoryPage {
                category: SearchCategory::Video,
                page: 1,
                total: Some(1),
                has_more: false,
                page_size: Some(20),
                total_pages: Some(1),
                items: vec![Candidate {
                    reference: ContentRef::Video(bili_core::Aid::new(42).unwrap()),
                    category: SearchCategory::Video,
                    title: "Readable title".into(),
                    cover: Some("fixture:cover".into()),
                    duration_ms: Some(213_000),
                    author: Some("Readable author".into()),
                    published_at: None,
                    plays: Some(123),
                    danmaku: None,
                }],
            }],
        });
        let (output, _) = render(&ctx, &mut state, &mut covers, vec![]);
        for label in [
            "番剧",
            "动画",
            "Readable title",
            "Readable author",
            "播放 123",
            "视频 · 3:33",
            "/ 1",
        ] {
            let (index, text) = output
                .shapes
                .iter()
                .enumerate()
                .find_map(|(index, shape)| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => Some((index, text)),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing label: {label}"));
            let bounds = text.galley.rect.translate(text.pos.to_vec2());
            assert!(output.shapes[..index].iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(rect)
                    if rect.fill == theme::card() && rect.fill.a() == 255 && rect.rect.contains_rect(bounds))
            }), "label has no opaque local backing: {label}");
        }
        let cover = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.fill_texture_id == texture => Some(rect.rect),
                _ => None,
            })
            .unwrap();
        assert!((cover.width() / cover.height() - 16.0 / 9.0).abs() < 0.001);
        let outside = egui::pos2(750.0, 630.0);
        assert!(
            !output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(rect)
                if rect.fill == theme::card() && rect.rect.contains(outside))
            }),
            "backing must not cover unused image space"
        );
        let click = cover.center();
        render(
            &ctx,
            &mut state,
            &mut covers,
            vec![
                egui::Event::PointerMoved(click),
                egui::Event::PointerButton {
                    pos: click,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let (_, action) = render(
            &ctx,
            &mut state,
            &mut covers,
            vec![egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(matches!(action, Some(Operation::Resolve(input)) if input == "av42"));
    }

    #[test]
    fn input_routes_without_validating_core_identifiers() {
        assert_eq!(classify_input("BV17x411w7KC"), InputKind::Content);
        assert_eq!(classify_input("风景"), InputKind::Keyword);
        assert_eq!(classify_input("  "), InputKind::Empty);
    }
    #[test]
    fn sort_resets_page_and_paging_uses_committed_query() {
        let mut state = SearchState {
            input: "风景".into(),
            ..Default::default()
        };
        assert!(matches!(
            state.choose_sort(SearchSort::Views),
            Some(Operation::Search(SearchQuery {
                page: 1,
                sort: SearchSort::Views,
                ..
            }))
        ));
        state.accept_results(CandidatePage { pages: vec![] });
        state.input = "未提交".into();
        let Some(Operation::Search(query)) = state.page(8) else {
            panic!("page action")
        };
        assert_eq!(query.keyword, "风景");
        assert_eq!(query.sort, SearchSort::Views);
        assert_eq!(query.page, 8);
        state.discard_request();
        assert!(matches!(
            state.choose_category(SearchCategory::Bangumi),
            Some(Operation::BrowseConditions(PgcSection::Anime))
        ));
    }
    #[test]
    fn cancellation_keeps_previous_results() {
        let mut state = SearchState {
            input: "原始".into(),
            ..Default::default()
        };
        state.submit_search();
        state.accept_results(CandidatePage { pages: vec![] });
        state.input = "新查询".into();
        state.choose_category(SearchCategory::Film);
        state.discard_request();
        state.mark_stale();
        assert_eq!(state.category, SearchCategory::Video);
        assert!(state.results.is_some());
        assert!(state.stale);
    }

    #[test]
    fn failed_initial_conditions_can_retry_from_the_tab_but_loaded_or_pending_tabs_do_not_refresh()
    {
        let mut state = SearchState::default();
        assert!(matches!(
            state.choose_category(SearchCategory::Bangumi),
            Some(Operation::BrowseConditions(PgcSection::Anime))
        ));
        state.start_wait();
        assert!(state.choose_category(SearchCategory::Bangumi).is_none());
        state.discard_request();
        state.mark_stale();
        assert!(matches!(
            state.choose_category(SearchCategory::Bangumi),
            Some(Operation::BrowseConditions(PgcSection::Anime))
        ));
        state.accept_conditions(BrowseConditions {
            section: PgcSection::Anime,
            filters: vec![],
            orders: vec![],
        });
        state.accept_results(CandidatePage { pages: vec![] });
        assert!(
            state.choose_category(SearchCategory::Bangumi).is_none(),
            "successful active categories never refresh on an ordinary click"
        );
    }

    #[test]
    fn pending_discovery_does_not_claim_an_empty_or_terminal_result() {
        fn has_text(output: &egui::FullOutput, needle: &str) -> bool {
            fn contains(shape: &egui::Shape, needle: &str) -> bool {
                match shape {
                    egui::Shape::Text(text) => text.galley.job.text == needle,
                    egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains(shape, needle)),
                    _ => false,
                }
            }
            output
                .shapes
                .iter()
                .any(|shape| contains(&shape.shape, needle))
        }
        let ctx = egui::Context::default();
        let mut state = SearchState::default();
        let mut covers = CoverCache::default();
        state.start_wait();
        state.started = Instant::now().checked_sub(Duration::from_secs(4));
        let (output, _) = render(&ctx, &mut state, &mut covers, vec![]);
        assert!(!has_text(
            &output,
            "搜索关键词，或粘贴视频、番剧与影视链接。"
        ));
        assert!(!has_text(&output, "没有找到相关内容。"));
        state.accept_results(CandidatePage { pages: vec![] });
        let (output, _) = render(&ctx, &mut state, &mut covers, vec![]);
        assert!(!state.pending());
        assert!(has_text(&output, "没有找到相关内容。"));
        state.start_wait();
        let (output, _) = render(&ctx, &mut state, &mut covers, vec![]);
        assert!(
            !has_text(&output, "没有找到相关内容。"),
            "an old empty result is not the new pending request's outcome"
        );
        state.discard_request();
        assert!(!state.pending());
        assert!(
            state.results.is_some(),
            "cancellation retains the committed result"
        );
    }

    #[test]
    fn pgc_common_filters_survive_section_switches_and_failed_requests_preserve_committed_choices()
    {
        use bili_core::{
            BrowseChoice, BrowseFilterField as Field, BrowseFilterGroup, BrowseOrderChoice,
        };
        fn conditions(section: PgcSection) -> BrowseConditions {
            let mut filters = vec![
                BrowseFilterGroup {
                    field: Field::StyleId,
                    label: "风格".into(),
                    choices: vec![
                        BrowseChoice {
                            value: "-1".into(),
                            label: "全部".into(),
                        },
                        BrowseChoice {
                            value: "10010".into(),
                            label: "原创".into(),
                        },
                    ],
                },
                BrowseFilterGroup {
                    field: Field::Year,
                    label: "年份".into(),
                    choices: vec![
                        BrowseChoice {
                            value: "-1".into(),
                            label: "全部".into(),
                        },
                        BrowseChoice {
                            value: "[2024,2025)".into(),
                            label: "2024".into(),
                        },
                    ],
                },
            ];
            if section == PgcSection::Anime {
                filters.push(BrowseFilterGroup {
                    field: Field::SpokenLanguageType,
                    label: "配音".into(),
                    choices: vec![
                        BrowseChoice {
                            value: "-1".into(),
                            label: "全部".into(),
                        },
                        BrowseChoice {
                            value: "1".into(),
                            label: "原声".into(),
                        },
                    ],
                });
            }
            BrowseConditions {
                section,
                filters,
                orders: vec![BrowseOrderChoice {
                    id: 3,
                    label: "追番人数".into(),
                    sorts: vec![0, 1],
                }],
            }
        }
        let mut state = SearchState::default();
        state.accept_conditions(conditions(PgcSection::Anime));
        let mut selected = BrowseQuery::new(PgcSection::Anime);
        selected.filters = [
            (Field::StyleId, "10010".into()),
            (Field::Year, "[2024,2025)".into()),
            (Field::SpokenLanguageType, "1".into()),
        ]
        .into();
        selected.order = Some(3);
        selected.sort = 1;
        selected.page = 3;
        state.set_requested_browse(selected.clone());
        state.accept_results(CandidatePage { pages: vec![] });
        let Operation::Browse(next) =
            state.accept_conditions(conditions(PgcSection::ChineseAnimation))
        else {
            panic!("expected browse");
        };
        assert_eq!(
            next.filters.get(&Field::StyleId).map(String::as_str),
            Some("10010")
        );
        assert_eq!(
            next.filters.get(&Field::Year).map(String::as_str),
            Some("[2024,2025)")
        );
        assert!(!next.filters.contains_key(&Field::SpokenLanguageType));
        assert!(state.filter_notice.as_ref().unwrap().contains("配音"));
        assert_eq!((next.page, next.order, next.sort), (1, Some(3), 1));
        state.discard_request();
        assert_eq!(state.browse.as_ref(), Some(&selected));
        state.accept_conditions(conditions(PgcSection::ChineseAnimation));
        state.accept_results(CandidatePage { pages: vec![] });
        let Some(Operation::Browse(page)) = state.page(2) else {
            panic!("expected next page");
        };
        assert_eq!(
            page.filters.get(&Field::StyleId).map(String::as_str),
            Some("10010")
        );
        assert_eq!(page.page, 2);
        state.discard_request();
        let Operation::Browse(restored) = state.accept_conditions(conditions(PgcSection::Anime))
        else {
            panic!("expected browse");
        };
        assert_eq!(restored.filters, selected.filters);
    }
}
