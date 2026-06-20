//! Search models, parsing, indexing, and ranking will live here.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::actions::Action;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchResultKind {
    Application,
    File,
    Directory,
    Calculator,
    WebSearch,
    Command,
    Feedback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub detail: Option<String>,
    pub kind: SearchResultKind,
    pub provider: String,
    pub score: i64,
    pub main_action: Action,
    pub secondary_actions: Vec<Action>,
    pub snippet: Option<SearchSnippet>,
}

impl SearchResult {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        kind: SearchResultKind,
        main_action: Action,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            detail: None,
            kind,
            provider: String::new(),
            score: 0,
            main_action,
            secondary_actions: Vec::new(),
            snippet: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_score(mut self, score: i64) -> Self {
        self.score = score;
        self
    }

    pub fn with_secondary_action(mut self, action: Action) -> Self {
        self.secondary_actions.push(action);
        self
    }

    pub fn with_snippet(mut self, snippet: SearchSnippet) -> Self {
        self.snippet = Some(snippet);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnippet {
    pub start_line: usize,
    pub lines: Vec<String>,
    pub highlights: Vec<SearchHighlight>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHighlight {
    pub line: usize,
    pub start_column: usize,
    pub end_column: usize,
    pub matched_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchMode {
    Normal,
    Regex,
    WebSearch { prefix: String },
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRequest {
    pub original: String,
    pub text: String,
    pub mode: SearchMode,
}

impl QueryRequest {
    pub fn new(text: impl Into<String>, mode: SearchMode) -> Self {
        let text = text.into();
        Self {
            original: text.clone(),
            text,
            mode,
        }
    }
}

#[derive(Debug, Default)]
pub struct SearchRequestTracker {
    latest_generation: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchRequestTicket {
    generation: u64,
}

impl SearchRequestTracker {
    pub fn begin(&self) -> SearchRequestTicket {
        let generation = self
            .latest_generation
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        SearchRequestTicket { generation }
    }

    pub fn is_latest(&self, ticket: SearchRequestTicket) -> bool {
        self.latest_generation.load(Ordering::Relaxed) == ticket.generation
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryParserConfig {
    pub regex_prefix: String,
    pub web_search_prefixes: Vec<String>,
    pub command_prefix: String,
}

impl Default for QueryParserConfig {
    fn default() -> Self {
        Self {
            regex_prefix: "re:".to_owned(),
            web_search_prefixes: Vec::new(),
            command_prefix: ">".to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct QueryParser {
    config: QueryParserConfig,
}

impl QueryParser {
    pub fn new(config: QueryParserConfig) -> Self {
        Self { config }
    }

    pub fn parse(&self, input: &str) -> QueryRequest {
        let trimmed = input.trim();

        if let Some(query) = trimmed.strip_prefix(&self.config.regex_prefix) {
            return QueryRequest {
                original: input.to_owned(),
                text: query.trim().to_owned(),
                mode: SearchMode::Regex,
            };
        }

        if let Some(query) = trimmed.strip_prefix(&self.config.command_prefix) {
            return QueryRequest {
                original: input.to_owned(),
                text: query.trim().to_owned(),
                mode: SearchMode::Command,
            };
        }

        for prefix in &self.config.web_search_prefixes {
            if let Some((candidate_prefix, query)) = trimmed.split_once(char::is_whitespace) {
                if candidate_prefix == prefix && !query.trim().is_empty() {
                    return QueryRequest {
                        original: input.to_owned(),
                        text: query.trim().to_owned(),
                        mode: SearchMode::WebSearch {
                            prefix: prefix.clone(),
                        },
                    };
                }
            }

            let legacy_marker = format!("{prefix}:");
            if let Some(query) = trimmed.strip_prefix(&legacy_marker) {
                return QueryRequest {
                    original: input.to_owned(),
                    text: query.trim().to_owned(),
                    mode: SearchMode::WebSearch {
                        prefix: prefix.clone(),
                    },
                };
            }
        }

        QueryRequest {
            original: input.to_owned(),
            text: trimmed.to_owned(),
            mode: SearchMode::Normal,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HistoryScores {
    scores: HashMap<String, i64>,
}

impl HistoryScores {
    pub fn from_pairs<const N: usize>(pairs: [(&str, i64); N]) -> Self {
        Self {
            scores: pairs
                .into_iter()
                .map(|(id, score)| (id.to_owned(), score))
                .collect(),
        }
    }

    pub fn get(&self, id: &str) -> i64 {
        self.scores.get(id).copied().unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct Ranker {
    application_type_weight: i64,
    file_type_weight: i64,
    exact_match_weight: i64,
    fuzzy_match_weight: i64,
    history_weight: i64,
    path_depth_penalty: i64,
}

impl Default for Ranker {
    fn default() -> Self {
        Self {
            application_type_weight: 500,
            file_type_weight: 250,
            exact_match_weight: 1_000,
            fuzzy_match_weight: 25,
            history_weight: 10,
            path_depth_penalty: 3,
        }
    }
}

impl Ranker {
    pub fn rank(
        &self,
        query: &str,
        mut results: Vec<SearchResult>,
        history: &HistoryScores,
    ) -> Vec<SearchResult> {
        for result in &mut results {
            result.score = self.score_result(query, result, history);
        }

        results.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.id.cmp(&right.id))
        });
        results
    }

    fn score_result(&self, query: &str, result: &SearchResult, history: &HistoryScores) -> i64 {
        let query = query.trim().to_lowercase();
        let title = result.title.to_lowercase();
        let detail = result.detail.as_deref().unwrap_or_default().to_lowercase();
        let haystack = format!("{title} {detail}");

        let mut score = 0;
        if !query.is_empty() && haystack.contains(&query) {
            score += self.exact_match_weight;
        } else if fuzzy_matches(&query, &haystack) {
            score += self.fuzzy_match_weight;
        }

        score += match result.kind {
            SearchResultKind::Application => self.application_type_weight,
            SearchResultKind::File => self.file_type_weight,
            SearchResultKind::Directory
            | SearchResultKind::Calculator
            | SearchResultKind::WebSearch
            | SearchResultKind::Command
            | SearchResultKind::Feedback => 0,
        };
        score += history.get(&result.id) * self.history_weight;
        score -= path_depth(result) * self.path_depth_penalty;
        score
    }
}

fn fuzzy_matches(query: &str, haystack: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    let mut chars = query.chars();
    let Some(mut current) = chars.next() else {
        return true;
    };

    for candidate in haystack.chars() {
        if candidate == current {
            match chars.next() {
                Some(next) => current = next,
                None => return true,
            }
        }
    }

    false
}

fn path_depth(result: &SearchResult) -> i64 {
    result
        .detail
        .as_deref()
        .unwrap_or_default()
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .count() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::actions::Action;

    #[test]
    fn search_result_has_display_fields_kind_and_actions() {
        let mut result = SearchResult::new(
            "file:/tmp/readme.md",
            "readme.md",
            SearchResultKind::File,
            Action::OpenPath {
                path: "/tmp/readme.md".to_owned(),
            },
        )
        .with_detail("/tmp/readme.md")
        .with_score(12)
        .with_secondary_action(Action::CopyText {
            text: "/tmp/readme.md".to_owned(),
        });

        result.provider = "files".to_owned();

        assert_eq!(result.id, "file:/tmp/readme.md");
        assert_eq!(result.title, "readme.md");
        assert_eq!(result.detail, Some("/tmp/readme.md".to_owned()));
        assert_eq!(result.kind, SearchResultKind::File);
        assert_eq!(result.provider, "files");
        assert_eq!(result.score, 12);
        assert!(result.snippet.is_none());
        assert_eq!(
            result.main_action,
            Action::OpenPath {
                path: "/tmp/readme.md".to_owned()
            }
        );
        assert_eq!(
            result.secondary_actions,
            vec![Action::CopyText {
                text: "/tmp/readme.md".to_owned()
            }]
        );
    }

    #[test]
    fn parser_treats_plain_text_as_normal_query() {
        let parser = QueryParser::new(QueryParserConfig::default());

        let request = parser.parse(" notes ");

        assert_eq!(request.original, " notes ");
        assert_eq!(request.text, "notes");
        assert_eq!(request.mode, SearchMode::Normal);
    }

    #[test]
    fn parser_uses_configured_regex_prefix() {
        let parser = QueryParser::new(QueryParserConfig {
            regex_prefix: "rx:".to_owned(),
            ..QueryParserConfig::default()
        });

        let request = parser.parse("rx:.*\\.pdf$");

        assert_eq!(request.text, ".*\\.pdf$");
        assert_eq!(request.mode, SearchMode::Regex);
    }

    #[test]
    fn parser_recognizes_configured_web_search_prefixes() {
        let parser = QueryParser::new(QueryParserConfig {
            web_search_prefixes: vec!["g".to_owned(), "gh".to_owned()],
            ..QueryParserConfig::default()
        });

        let request = parser.parse("gh tauri plugins");

        assert_eq!(request.text, "tauri plugins");
        assert_eq!(
            request.mode,
            SearchMode::WebSearch {
                prefix: "gh".to_owned()
            }
        );
    }

    #[test]
    fn parser_recognizes_space_separated_web_search_prefixes() {
        let parser = QueryParser::new(QueryParserConfig {
            web_search_prefixes: vec!["g".to_owned(), "bd".to_owned()],
            ..QueryParserConfig::default()
        });

        let google = parser.parse("g 1234");
        let baidu = parser.parse("bd 1234");
        let prefix_only = parser.parse("g ");

        assert_eq!(google.text, "1234");
        assert_eq!(
            google.mode,
            SearchMode::WebSearch {
                prefix: "g".to_owned()
            }
        );
        assert_eq!(baidu.text, "1234");
        assert_eq!(
            baidu.mode,
            SearchMode::WebSearch {
                prefix: "bd".to_owned()
            }
        );
        assert_eq!(prefix_only.mode, SearchMode::Normal);
    }

    #[test]
    fn parser_recognizes_command_prefix() {
        let parser = QueryParser::new(QueryParserConfig::default());

        let request = parser.parse("> git status");

        assert_eq!(request.text, "git status");
        assert_eq!(request.mode, SearchMode::Command);
    }

    #[test]
    fn search_request_tracker_marks_older_requests_stale() {
        let tracker = SearchRequestTracker::default();

        let first = tracker.begin();
        assert!(tracker.is_latest(first));

        let second = tracker.begin();
        assert!(!tracker.is_latest(first));
        assert!(tracker.is_latest(second));
    }

    #[test]
    fn ranker_prefers_exact_substring_matches_over_weaker_fuzzy_matches() {
        let results = vec![
            file_result("fuzzy", "rdme.txt", "/home/frank/rdme.txt"),
            file_result("exact", "readme.md", "/home/frank/readme.md"),
        ];

        let ranked = Ranker::default().rank("readme", results, &HistoryScores::default());

        assert_eq!(ranked[0].id, "exact");
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn ranker_prefers_shallower_paths_when_match_quality_is_similar() {
        let results = vec![
            file_result("deep", "notes.md", "/home/frank/projects/archive/notes.md"),
            file_result("shallow", "notes.md", "/home/frank/notes.md"),
        ];

        let ranked = Ranker::default().rank("notes", results, &HistoryScores::default());

        assert_eq!(ranked[0].id, "shallow");
    }

    #[test]
    fn ranker_uses_history_to_boost_recently_used_results() {
        let results = vec![
            file_result("unused", "notes.md", "/home/frank/notes.md"),
            file_result("recent", "notes.md", "/home/frank/archive/notes.md"),
        ];
        let history = HistoryScores::from_pairs([("recent", 50)]);

        let ranked = Ranker::default().rank("notes", results, &history);

        assert_eq!(ranked[0].id, "recent");
    }

    #[test]
    fn ranker_prefers_applications_then_files_then_directories_when_quality_is_similar() {
        let results = vec![
            directory_result("directory", "QuickFox", "/home/frank/QuickFox"),
            file_result("file", "QuickFox", "/home/frank/QuickFox.txt"),
            application_result("app", "QuickFox", "/Applications/QuickFox.app"),
        ];

        let ranked = Ranker::default().rank("QuickFox", results, &HistoryScores::default());

        assert_eq!(ranked[0].id, "app");
        assert_eq!(ranked[1].id, "file");
        assert_eq!(ranked[2].id, "directory");
    }

    fn file_result(id: &str, title: &str, path: &str) -> SearchResult {
        SearchResult::new(
            id,
            title,
            SearchResultKind::File,
            Action::OpenPath {
                path: path.to_owned(),
            },
        )
        .with_detail(path)
    }

    fn directory_result(id: &str, title: &str, path: &str) -> SearchResult {
        SearchResult::new(
            id,
            title,
            SearchResultKind::Directory,
            Action::OpenPath {
                path: path.to_owned(),
            },
        )
        .with_detail(path)
    }

    fn application_result(id: &str, title: &str, path: &str) -> SearchResult {
        SearchResult::new(
            id,
            title,
            SearchResultKind::Application,
            Action::OpenPath {
                path: path.to_owned(),
            },
        )
        .with_detail(path)
    }
}
