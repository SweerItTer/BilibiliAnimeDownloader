//! Explicit discovery submission and presentation of core-owned results.
use crate::{
    backend::{Bridge, Operation},
    covers::CoverCache,
    theme::{self, ButtonKind, Icon},
};
use bili_core::{CandidatePage, ContentRef, SearchCategory, SearchQuery};
use egui::{RichText, Ui};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Empty,
    Keyword,
    Content,
}

/// This only chooses the UI route. Core remains the identifier/URL validator.
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
        SearchCategory::All => "全部",
        SearchCategory::Video => "视频",
        SearchCategory::Bangumi => "番剧",
        SearchCategory::Film => "影视",
    }
}

pub fn duration(ms: Option<u64>) -> String {
    match ms {
        Some(ms) => {
            let seconds = ms / 1000;
            if seconds >= 3600 {
                format!(
                    "{}:{:02}:{:02}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                )
            } else {
                format!("{}:{:02}", seconds / 60, seconds % 60)
            }
        }
        None => "时长未知".into(),
    }
}

pub struct SearchState {
    pub input: String,
    category: SearchCategory,
    pub results: Option<CandidatePage>,
    submitted: Option<SearchQuery>,
    requested: Option<SearchQuery>,
    pub request_focus: bool,
    pub stale: bool,
    started: Option<Instant>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            input: String::new(),
            category: SearchCategory::All,
            results: None,
            submitted: None,
            requested: None,
            request_focus: false,
            stale: false,
            started: None,
        }
    }
}

impl SearchState {
    pub fn accept_results(&mut self, results: CandidatePage) {
        self.results = Some(results);
        self.submitted = self.requested.take();
        self.stale = false;
        self.started = None;
    }

    pub fn discard_request(&mut self) {
        self.requested = None;
        self.started = None;
    }

    pub fn requested_query(&self) -> Option<&SearchQuery> {
        self.requested.as_ref()
    }

    pub fn set_requested(&mut self, query: SearchQuery) {
        self.requested = Some(query);
    }

    pub fn start_wait(&mut self) {
        self.started = Some(Instant::now());
    }

    pub fn waited(&self) -> Option<Duration> {
        self.started.map(|started| started.elapsed())
    }

    pub fn mark_stale(&mut self) {
        if self.results.is_some() {
            self.stale = true;
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        available: bool,
        reduced: bool,
        covers: &mut CoverCache,
        bridge: Option<&Bridge>,
    ) -> Option<Operation> {
        let mut action = None;
        theme::card_frame().show(ui, |ui| {
            theme::inset(theme::card()).show(ui, |ui| {
                let mut enter = false;
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 140.0).max(120.0);
                    let response = ui.add_sized(
                        [width, 42.0],
                        egui::TextEdit::singleline(&mut self.input)
                            .hint_text("搜索关键词，或粘贴 BV / AV / ss / ep / 链接")
                            .font(egui::FontId::proportional(15.0))
                            .margin(egui::vec2(12.0, 11.0)),
                    );
                    if self.request_focus {
                        response.request_focus();
                        self.request_focus = false;
                    }
                    enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let content = classify_input(&self.input) == InputKind::Content;
                    ui.add_enabled_ui(
                        available && classify_input(&self.input) != InputKind::Empty,
                        |ui| {
                            if theme::button(
                                ui,
                                if content {
                                    "解析链接"
                                } else {
                                    "开始搜索"
                                },
                                Icon::Search,
                                ButtonKind::Primary,
                                reduced,
                            )
                            .clicked()
                            {
                                enter = true;
                            }
                        },
                    );
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(available, |ui| {
                        for category in [
                            SearchCategory::All,
                            SearchCategory::Video,
                            SearchCategory::Bangumi,
                            SearchCategory::Film,
                        ] {
                            let selected = self.category == category;
                            if theme::button(
                                ui,
                                category_name(category),
                                if selected { Icon::Check } else { Icon::Play },
                                if selected {
                                    ButtonKind::Soft
                                } else {
                                    ButtonKind::Quiet
                                },
                                reduced,
                            )
                            .clicked()
                            {
                                self.category = category;
                            }
                        }
                    });
                    theme::muted(ui, "仅在提交后联网");
                });
                if enter && available {
                    match classify_input(&self.input) {
                        InputKind::Content => {
                            action = Some(Operation::Resolve(self.input.trim().to_owned()))
                        }
                        InputKind::Keyword => {
                            let query = SearchQuery {
                                keyword: self.input.trim().to_owned(),
                                category: self.category,
                                page: 1,
                            };
                            self.requested = Some(query.clone());
                            action = Some(Operation::Search(query));
                        }
                        InputKind::Empty => {}
                    }
                }
            });
        });
        ui.add_space(14.0);
        if let Some(results) = &self.results {
            let count: usize = results.pages.iter().map(|p| p.items.len()).sum();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("搜索结果").size(18.0).strong());
                if let Some(query) = &self.submitted {
                    theme::muted(
                        ui,
                        format!(
                            "{} · {} · 第 {} 页 · {} 项",
                            category_name(query.category),
                            query.keyword,
                            query.page,
                            count
                        ),
                    );
                }
                if self.stale {
                    ui.label(
                        RichText::new("上次成功结果，本次筛选未更新")
                            .size(12.0)
                            .color(theme::ERROR),
                    )
                    .on_hover_text(
                        "新一次搜索失败或被取消，下面仍是上一组结果。可点击顶部横幅的重试。",
                    );
                }
            });
            ui.add_space(6.0);
            if count == 0 {
                empty(
                    ui,
                    "没有找到相关内容",
                    "换一个关键词，或直接粘贴视频与番剧链接。",
                );
            } else {
                for page in &results.pages {
                    for (index, item) in page.items.iter().enumerate() {
                        ui.push_id((category_name(page.category), index), |ui| {
                            theme::card_frame().show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let size = egui::vec2(160.0, 90.0);
                                    if !covers.show(ui, bridge, item.cover.as_deref(), size, 14.0) {
                                        let (rect, _) =
                                            ui.allocate_exact_size(size, egui::Sense::hover());
                                        ui.painter().rect_filled(
                                            rect,
                                            14.0,
                                            if page.category == SearchCategory::Video {
                                                theme::soft()
                                            } else {
                                                theme::rose()
                                            },
                                        );
                                        theme::icon(
                                            ui.painter(),
                                            egui::Rect::from_center_size(
                                                rect.center(),
                                                egui::vec2(28.0, 28.0),
                                            ),
                                            Icon::Play,
                                            theme::primary(),
                                        );
                                    }
                                    ui.add_space(4.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2((ui.available_width() - 130.0).max(120.0), 90.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.add_space(14.0);
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&item.title).size(16.0).strong(),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(&item.title);
                                            ui.add_space(4.0);
                                            theme::muted(
                                                ui,
                                                format!(
                                                    "{}  ·  {}",
                                                    category_name(item.category),
                                                    duration(item.duration_ms)
                                                ),
                                            );
                                        },
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.add_enabled_ui(available, |ui| {
                                                if theme::button(
                                                    ui,
                                                    "查看选集",
                                                    Icon::Arrow,
                                                    ButtonKind::Soft,
                                                    reduced,
                                                )
                                                .clicked()
                                                {
                                                    action = Some(Operation::Resolve(
                                                        reference_input(&item.reference),
                                                    ));
                                                }
                                            });
                                        },
                                    );
                                });
                            });
                            ui.add_space(6.0);
                        });
                    }
                }
            }
            let more = results.pages.iter().any(|p| p.has_more);
            if let Some(query) = &self.submitted {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(available && query.page > 1, |ui| {
                        if theme::button(ui, "上一页", Icon::Back, ButtonKind::Quiet, reduced)
                            .clicked()
                        {
                            let mut next = query.clone();
                            next.page -= 1;
                            self.requested = Some(next.clone());
                            action = Some(Operation::Search(next));
                        }
                    });
                    ui.add_enabled_ui(available && more && query.page < u32::MAX, |ui| {
                        if theme::button(ui, "下一页", Icon::Arrow, ButtonKind::Soft, reduced)
                            .clicked()
                        {
                            let mut next = query.clone();
                            next.page += 1;
                            self.requested = Some(next.clone());
                            action = Some(Operation::Search(next));
                        }
                    });
                });
            }
        } else {
            theme::card_frame().show(ui, |ui| {
                theme::inset(theme::card()).show(ui, |ui| {
                    ui.set_min_height(220.0);
                    ui.vertical_centered(|ui| {
                        theme::empty_art(ui);
                        ui.label(RichText::new("把喜欢的内容，好好收藏").size(24.0).strong());
                        theme::muted(ui, "从一个关键词或链接开始，让下载变得简单。");
                        ui.add_space(18.0);
                        ui.horizontal_wrapped(|ui| {
                            let width = ((ui.available_width() - 20.0) / 3.0).max(130.0);
                            for (number, title, subtitle) in [
                                ("01", "搜索或粘贴", "关键词、视频编号、链接"),
                                ("02", "选择想看的", "按集选择，自动匹配画质"),
                                ("03", "交给下载队列", "原生处理，随时暂停"),
                            ] {
                                theme::inset(theme::background()).show(ui, |ui| {
                                    ui.set_width((width - 32.0).max(90.0));
                                    ui.label(
                                        RichText::new(number)
                                            .color(theme::primary())
                                            .size(12.0)
                                            .strong(),
                                    );
                                    ui.label(RichText::new(title).strong());
                                    ui.label(
                                        RichText::new(subtitle)
                                            .color(theme::muted_color())
                                            .size(12.0),
                                    );
                                });
                            }
                        });
                    });
                });
            });
        }
        action
    }
}

pub fn empty(ui: &mut Ui, title: &str, subtitle: &str) {
    theme::card_frame().show(ui, |ui| {
        theme::inset(theme::card()).show(ui, |ui| {
            ui.set_min_height(220.0);
            ui.vertical_centered(|ui| {
                ui.add_space(14.0);
                theme::empty_art(ui);
                ui.label(RichText::new(title).size(23.0).strong());
                theme::muted(ui, subtitle);
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classification_routes_without_reimplementing_core_validation() {
        assert_eq!(classify_input(" \n "), InputKind::Empty);
        for input in [
            "BV17x411w7KC",
            "av170001",
            "ss123",
            "ep456",
            " https://b23.tv/test ",
        ] {
            assert_eq!(classify_input(input), InputKind::Content);
        }
        for input in ["动画", "avatar", "episode one", "av作品"] {
            assert_eq!(classify_input(input), InputKind::Keyword);
        }
        assert_eq!(
            classify_input("https://untrusted.example/"),
            InputKind::Content
        );
    }
    #[test]
    fn duration_formats_hours_and_unknown_values() {
        assert_eq!(duration(None), "时长未知");
        assert_eq!(duration(Some(65_000)), "1:05");
        assert_eq!(duration(Some(3_661_000)), "1:01:01");
    }

    #[test]
    fn stale_marking_requires_previous_results_and_clears_on_accept() {
        let mut state = SearchState::default();
        state.mark_stale();
        assert!(!state.stale, "nothing to label as stale");
        state.results = Some(CandidatePage { pages: Vec::new() });
        state.mark_stale();
        assert!(state.stale);
        state.start_wait();
        assert!(state.waited().is_some());
        state.accept_results(CandidatePage { pages: Vec::new() });
        assert!(!state.stale);
        assert!(state.waited().is_none());
    }

    #[test]
    fn cancelled_pagination_keeps_the_last_successful_page() {
        let mut state = SearchState {
            requested: Some(SearchQuery {
                keyword: "动画".into(),
                category: SearchCategory::Bangumi,
                page: 1,
            }),
            ..SearchState::default()
        };
        state.accept_results(CandidatePage { pages: Vec::new() });
        state.requested = Some(SearchQuery {
            keyword: "动画".into(),
            category: SearchCategory::Bangumi,
            page: 2,
        });
        state.discard_request();
        assert_eq!(state.submitted.as_ref().unwrap().page, 1);
        assert!(state.requested.is_none());
        assert!(state.results.is_some());
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use bili_core::{Aid, Candidate, CategoryPage, ContentRef, SearchCategory};
    use egui::{Pos2, Rect, Vec2};

    fn texts(output: &egui::FullOutput) -> Vec<(String, Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                    out.push((text.galley.job.text.clone(), rect));
                }
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

    fn results() -> CandidatePage {
        CandidatePage {
            pages: vec![CategoryPage {
                category: SearchCategory::Video,
                page: 1,
                total: Some(4),
                has_more: false,
                items: (0..4)
                    .map(|index| Candidate {
                        reference: ContentRef::Video(Aid::new(1_000 + index).unwrap()),
                        category: SearchCategory::Video,
                        title: match index {
                            0 => "很短的标题".to_owned(),
                            1 => "中等长度的视频标题，用来检查卡片宽度是否一致".to_owned(),
                            _ => "这是一个非常非常长的视频标题，用来验证卡片在长文本下仍然保持统一宽度与布局".to_owned(),
                        },
                        cover: None,
                        duration_ms: Some(61_000),
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn result_cards_share_the_same_right_edge_without_overflow() {
        let size = Vec2::new(1100.0, 900.0);
        let screen = Rect::from_min_size(Pos2::ZERO, size);
        let ctx = egui::Context::default();
        let mut state = SearchState {
            input: "test".into(),
            results: Some(results()),
            ..SearchState::default()
        };
        let mut covers = CoverCache::default();
        let mut output = None;
        for _ in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            output = Some(ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let _ = state.show(ui, false, false, &mut covers, None);
                        });
                });
            }));
        }
        let output = output.unwrap();
        let all = texts(&output);
        let edges: Vec<f32> = all
            .iter()
            .filter(|(text, _)| text == "查看选集")
            .map(|(_, rect)| rect.right())
            .collect();
        assert_eq!(edges.len(), 4, "expected one action button per result");
        let widest = edges.iter().copied().fold(f32::MIN, f32::max);
        for edge in &edges {
            assert!(
                (widest - edge).abs() < 1.0,
                "action buttons are not aligned: {edges:?}"
            );
        }
        for (text, rect) in &all {
            assert!(
                rect.right() <= screen.right() + 1.0,
                "{text} overflows horizontally: {rect:?}"
            );
        }
    }
}
