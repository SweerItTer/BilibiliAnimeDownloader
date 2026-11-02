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
use std::time::{Duration, Instant};

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
            home_requested: false,
        }
    }
}
impl SearchState {
    pub fn accept_results(&mut self, results: CandidatePage) {
        self.browse = self.requested_browse.take();
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
    pub fn waited(&self) -> Option<Duration> {
        self.started.map(|s| s.elapsed())
    }
    pub fn mark_stale(&mut self) {
        self.stale = self.results.is_some();
    }
    pub fn choose_category(&mut self, category: SearchCategory) -> Option<Operation> {
        self.home_requested = true;
        if category == self.category {
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
        let query = BrowseQuery {
            section: conditions.section,
            genre: None,
            page: 1,
            page_size: 20,
        };
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
                        let current = self
                            .requested_browse
                            .as_ref()
                            .or(self.browse.as_ref())
                            .and_then(|q| q.genre);
                        let mut next = current;
                        let mut changed = false;
                        ui.horizontal_wrapped(|ui| {
                            if ui.selectable_label(current.is_none(), "全部类型").clicked() {
                                next = None;
                                changed = true;
                            }
                            for genre in &conditions.genres {
                                if ui
                                    .selectable_label(current == Some(genre.id), &genre.label)
                                    .clicked()
                                {
                                    next = Some(genre.id);
                                    changed = true;
                                }
                            }
                        });
                        if changed {
                            let query = BrowseQuery {
                                section: conditions.section,
                                genre: next,
                                page: 1,
                                page_size: 20,
                            };
                            self.requested = None;
                            self.requested_browse = Some(query.clone());
                            action = Some(Operation::Browse(query));
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
                            egui::Button::new(sort_name(sort)).selected(self.sort == sort),
                        )
                        .clicked()
                    {
                        action = self.choose_sort(sort);
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(if self.bulk {
                        "退出多选"
                    } else {
                        "批量下载"
                    })
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
        ui.add_space(12.0);
        if self.stale {
            ui.colored_label(theme::ERROR, "本次搜索未更新，以下保留上次结果。");
        }
        let items: Vec<Candidate> = self
            .results
            .as_ref()
            .map(|r| {
                r.pages
                    .iter()
                    .flat_map(|p| p.items.iter().cloned())
                    .collect()
            })
            .unwrap_or_default();
        if self.results.is_none() {
            theme::muted(ui, "搜索关键词，或粘贴视频、番剧与影视链接。");
        } else if items.is_empty() {
            theme::muted(ui, "没有找到相关内容。");
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
                            let changed = ui.checkbox(&mut check, "选择此内容").changed();
                            if response.clicked() || title.clicked() || changed {
                                if selected {
                                    self.selected.retain(|c| c.reference != item.reference);
                                } else {
                                    self.selected.push(item.clone());
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
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(available && current > 1, egui::Button::new("<"))
                    .clicked()
                {
                    action = self.page(current - 1);
                }
                let mut navigate = None;
                if let Some(draft) = &mut self.page_edit {
                    let response = ui.add_sized(
                        [48.0, 30.0],
                        theme::singleline_input(ui, draft, "", 30.0).id_salt("page-number"),
                    );
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
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
                    ui.colored_label(theme::ERROR, error);
                }
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
}
