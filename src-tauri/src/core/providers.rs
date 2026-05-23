//! Query providers will live here.

use crate::core::actions::Action;
use crate::core::index::SearchIndex;
use crate::core::search::{QueryRequest, SearchMode, SearchResult, SearchResultKind};

pub trait Provider {
    fn id(&self) -> &'static str;
    fn search(&self, query: &QueryRequest) -> Vec<SearchResult>;
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: Vec<Box<dyn Provider>>,
}

impl ProviderRegistry {
    pub fn register<P>(&mut self, provider: P)
    where
        P: Provider + 'static,
    {
        self.providers.push(Box::new(provider));
    }

    pub fn provider_ids(&self) -> Vec<&'static str> {
        self.providers
            .iter()
            .map(|provider| provider.id())
            .collect()
    }

    pub fn search(&self, query: &QueryRequest) -> Vec<SearchResult> {
        self.providers
            .iter()
            .flat_map(|provider| {
                provider.search(query).into_iter().map(|mut result| {
                    if result.provider.is_empty() {
                        result.provider = provider.id().to_owned();
                    }
                    result
                })
            })
            .collect()
    }
}

pub struct FileProvider {
    index: SearchIndex,
}

impl FileProvider {
    pub fn new(index: SearchIndex) -> Self {
        Self { index }
    }
}

impl Provider for FileProvider {
    fn id(&self) -> &'static str {
        "files"
    }

    fn search(&self, query: &QueryRequest) -> Vec<SearchResult> {
        self.index
            .search(query)
            .into_iter()
            .map(|mut result| {
                result.provider = self.id().to_owned();
                result
            })
            .collect()
    }
}

pub struct CalculatorProvider;

impl Provider for CalculatorProvider {
    fn id(&self) -> &'static str {
        "calculator"
    }

    fn search(&self, query: &QueryRequest) -> Vec<SearchResult> {
        if !matches!(query.mode, SearchMode::Normal) {
            return Vec::new();
        }

        let Some(value) = evaluate_calculator_expression(&query.text) else {
            return Vec::new();
        };
        let formatted = format_number(value);

        let mut result = SearchResult::new(
            format!("calculator:{}", query.text),
            formatted.clone(),
            SearchResultKind::Calculator,
            Action::CopyText { text: formatted },
        )
        .with_detail(query.text.clone())
        .with_score(900);
        result.provider = self.id().to_owned();
        vec![result]
    }
}

fn evaluate_calculator_expression(input: &str) -> Option<f64> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !looks_like_calculator_expression(trimmed) {
        return None;
    }

    let normalized = normalize_expression(trimmed)?;
    let mut parser = ExpressionParser::new(&normalized);
    let value = parser.parse_expression().ok()?;
    if parser.is_finished() && value.is_finite() {
        Some(value)
    } else {
        None
    }
}

fn looks_like_calculator_expression(input: &str) -> bool {
    input.contains(|character: char| {
        matches!(
            character,
            '0'..='9' | '+' | '-' | '*' | '/' | '^' | '(' | ')' | '.' | '%'
        )
    }) && input.chars().all(|character| {
        character.is_ascii_hexdigit()
            || character.is_ascii_whitespace()
            || matches!(
                character,
                '+' | '-'
                    | '*'
                    | '/'
                    | '^'
                    | '('
                    | ')'
                    | '.'
                    | '%'
                    | 'x'
                    | 'X'
                    | 's'
                    | 'q'
                    | 'r'
                    | 't'
            )
    })
}

fn normalize_expression(input: &str) -> Option<String> {
    let mut output = input.to_owned();
    while let Some(start) = output.find("0x").or_else(|| output.find("0X")) {
        let end = output[start + 2..]
            .find(|character: char| !character.is_ascii_hexdigit())
            .map(|offset| start + 2 + offset)
            .unwrap_or(output.len());
        let literal = &output[start + 2..end];
        let value = i64::from_str_radix(literal, 16).ok()?;
        output.replace_range(start..end, &value.to_string());
    }
    Some(output.replace('%', "/100"))
}

fn format_number(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        let mut formatted = format!("{value:.10}");
        while formatted.contains('.') && formatted.ends_with('0') {
            formatted.pop();
        }
        formatted.trim_end_matches('.').to_owned()
    }
}

struct ExpressionParser<'a> {
    chars: Vec<char>,
    position: usize,
    source: &'a str,
}

impl<'a> ExpressionParser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            chars: source.chars().collect(),
            position: 0,
            source,
        }
    }

    fn parse_expression(&mut self) -> Result<f64, ()> {
        let mut value = self.parse_term()?;
        loop {
            self.skip_whitespace();
            if self.consume('+') {
                value += self.parse_term()?;
            } else if self.consume('-') {
                value -= self.parse_term()?;
            } else {
                break;
            }
        }
        Ok(value)
    }

    fn parse_term(&mut self) -> Result<f64, ()> {
        let mut value = self.parse_power()?;
        loop {
            self.skip_whitespace();
            if self.consume('*') {
                value *= self.parse_power()?;
            } else if self.consume('/') {
                value /= self.parse_power()?;
            } else {
                break;
            }
        }
        Ok(value)
    }

    fn parse_power(&mut self) -> Result<f64, ()> {
        let mut value = self.parse_factor()?;
        self.skip_whitespace();
        if self.consume('^') {
            value = value.powf(self.parse_power()?);
        }
        Ok(value)
    }

    fn parse_factor(&mut self) -> Result<f64, ()> {
        self.skip_whitespace();
        if self.consume('-') {
            return Ok(-self.parse_factor()?);
        }
        if self.consume('(') {
            let value = self.parse_expression()?;
            self.expect(')')?;
            return Ok(value);
        }
        if self.remaining().starts_with("sqrt") {
            self.position += 4;
            self.expect('(')?;
            let value = self.parse_expression()?;
            self.expect(')')?;
            return Ok(value.sqrt());
        }
        self.parse_number()
    }

    fn parse_number(&mut self) -> Result<f64, ()> {
        self.skip_whitespace();
        let start = self.position;
        while self
            .chars
            .get(self.position)
            .is_some_and(|character| character.is_ascii_digit() || *character == '.')
        {
            self.position += 1;
        }

        if start == self.position {
            return Err(());
        }

        self.source[start..self.position]
            .parse::<f64>()
            .map_err(|_| ())
    }

    fn is_finished(&mut self) -> bool {
        self.skip_whitespace();
        self.position == self.chars.len()
    }

    fn remaining(&self) -> &str {
        &self.source[self.position..]
    }

    fn skip_whitespace(&mut self) {
        while self
            .chars
            .get(self.position)
            .is_some_and(|character| character.is_ascii_whitespace())
        {
            self.position += 1;
        }
    }

    fn consume(&mut self, expected: char) -> bool {
        self.skip_whitespace();
        if self.chars.get(self.position) == Some(&expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: char) -> Result<(), ()> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchEngine {
    pub prefix: String,
    pub name: String,
    pub url_template: String,
}

pub struct WebSearchProvider {
    engines: Vec<WebSearchEngine>,
}

impl WebSearchProvider {
    pub fn new(engines: Vec<WebSearchEngine>) -> Self {
        Self { engines }
    }
}

impl Provider for WebSearchProvider {
    fn id(&self) -> &'static str {
        "web-search"
    }

    fn search(&self, query: &QueryRequest) -> Vec<SearchResult> {
        let SearchMode::WebSearch { prefix } = &query.mode else {
            return Vec::new();
        };
        let Some(engine) = self.engines.iter().find(|engine| engine.prefix == *prefix) else {
            return Vec::new();
        };

        let url = engine
            .url_template
            .replace("{query}", &url_encode(&query.text));
        let mut result = SearchResult::new(
            format!("web:{}:{}", engine.prefix, query.text),
            format!("{}: {}", engine.name, query.text),
            SearchResultKind::WebSearch,
            Action::OpenUrl { url },
        )
        .with_detail(engine.url_template.clone())
        .with_score(800);
        result.provider = self.id().to_owned();
        vec![result]
    }
}

fn url_encode(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandProviderConfig {
    pub enabled: bool,
    pub prefix: String,
}

pub struct CommandProvider {
    config: CommandProviderConfig,
}

impl CommandProvider {
    pub fn new(config: CommandProviderConfig) -> Self {
        Self { config }
    }
}

impl Provider for CommandProvider {
    fn id(&self) -> &'static str {
        "commands"
    }

    fn search(&self, query: &QueryRequest) -> Vec<SearchResult> {
        if !matches!(query.mode, SearchMode::Command) || query.text.trim().is_empty() {
            return Vec::new();
        }

        if !self.config.enabled {
            let mut result = SearchResult::new(
                "commands:disabled",
                "启用命令执行后才能运行命令",
                SearchResultKind::Feedback,
                Action::CopyText {
                    text: "命令执行默认关闭，请在设置中启用。".to_owned(),
                },
            )
            .with_detail(format!("{} {}", self.config.prefix, query.text))
            .with_score(700);
            result.provider = self.id().to_owned();
            return vec![result];
        }

        let command = query.text.trim().to_owned();
        let mut result = SearchResult::new(
            format!("command:{command}"),
            command.clone(),
            SearchResultKind::Command,
            Action::ExecuteCommand {
                command: command.clone(),
                requires_confirmation: true,
            },
        )
        .with_detail("确认后在外部终端执行".to_owned())
        .with_score(850);
        result.provider = self.id().to_owned();
        vec![result]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index::{IndexedEntry, IndexedEntryKind, SearchIndex};

    struct StaticProvider {
        provider_id: &'static str,
        result_title: &'static str,
    }

    impl Provider for StaticProvider {
        fn id(&self) -> &'static str {
            self.provider_id
        }

        fn search(&self, _query: &QueryRequest) -> Vec<SearchResult> {
            vec![SearchResult::new(
                format!("{}:{}", self.provider_id, self.result_title),
                self.result_title,
                SearchResultKind::File,
                Action::OpenPath {
                    path: format!("/tmp/{}", self.result_title),
                },
            )]
        }
    }

    #[test]
    fn registry_queries_all_registered_providers_and_merges_results() {
        let mut registry = ProviderRegistry::default();
        registry.register(StaticProvider {
            provider_id: "files",
            result_title: "notes.md",
        });
        registry.register(StaticProvider {
            provider_id: "calculator",
            result_title: "1024",
        });

        let results = registry.search(&QueryRequest::new("2^10", SearchMode::Normal));

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "files:notes.md");
        assert_eq!(results[1].id, "calculator:1024");
    }

    #[test]
    fn registry_exposes_provider_ids_in_registration_order() {
        let mut registry = ProviderRegistry::default();
        registry.register(StaticProvider {
            provider_id: "files",
            result_title: "notes.md",
        });
        registry.register(StaticProvider {
            provider_id: "web",
            result_title: "Search web",
        });

        assert_eq!(registry.provider_ids(), vec!["files", "web"]);
    }

    #[test]
    fn file_provider_returns_file_and_directory_results_with_open_actions() {
        let index = SearchIndex::from_entries(vec![
            indexed_entry(
                "/home/frank/Documents",
                "Documents",
                IndexedEntryKind::Directory,
            ),
            indexed_entry(
                "/home/frank/Documents/readme.md",
                "readme.md",
                IndexedEntryKind::File,
            ),
        ]);
        let provider = FileProvider::new(index);

        let results = provider.search(&QueryRequest::new("doc", SearchMode::Normal));

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.provider == "files"));
        assert!(results.iter().any(|result| {
            result.title == "readme.md"
                && result.kind == SearchResultKind::File
                && result.main_action
                    == Action::OpenPath {
                        path: "/home/frank/Documents/readme.md".to_owned(),
                    }
        }));
        assert!(results.iter().any(|result| {
            result.title == "Documents"
                && result.kind == SearchResultKind::Directory
                && result.main_action
                    == Action::OpenPath {
                        path: "/home/frank/Documents".to_owned(),
                    }
        }));
        assert!(results
            .iter()
            .all(|result| result.secondary_actions.len() == 2));
        assert!(results.iter().any(|result| {
            result
                .secondary_actions
                .contains(&Action::OpenContainingFolder {
                    path: result.detail.clone().unwrap_or_default(),
                })
                && result.secondary_actions.contains(&Action::CopyText {
                    text: result.detail.clone().unwrap_or_default(),
                })
        }));
    }

    #[test]
    fn file_provider_can_be_registered_with_provider_registry() {
        let index = SearchIndex::from_entries(vec![indexed_entry(
            "/home/frank/report.pdf",
            "report.pdf",
            IndexedEntryKind::File,
        )]);
        let mut registry = ProviderRegistry::default();
        registry.register(FileProvider::new(index));

        let results = registry.search(&QueryRequest::new("report", SearchMode::Normal));

        assert_eq!(registry.provider_ids(), vec!["files"]);
        assert_eq!(results[0].title, "report.pdf");
        assert_eq!(results[0].provider, "files");
    }

    #[test]
    fn calculator_provider_evaluates_enhanced_expressions() {
        let provider = CalculatorProvider;

        let cases = [
            ("2^10", "1024"),
            ("sqrt(9)", "3"),
            ("0xff", "255"),
            ("50%", "0.5"),
        ];

        for (input, expected) in cases {
            let results = provider.search(&QueryRequest::new(input, SearchMode::Normal));

            assert_eq!(results.len(), 1, "{input}");
            assert_eq!(results[0].title, expected, "{input}");
            assert_eq!(results[0].provider, "calculator");
            assert_eq!(results[0].kind, SearchResultKind::Calculator);
            assert_eq!(
                results[0].main_action,
                Action::CopyText {
                    text: expected.to_owned()
                }
            );
        }
    }

    #[test]
    fn calculator_provider_ignores_non_calculator_queries() {
        let provider = CalculatorProvider;

        let results = provider.search(&QueryRequest::new("readme", SearchMode::Normal));

        assert!(results.is_empty());
    }

    #[test]
    fn web_search_provider_generates_url_for_configured_prefix_and_encodes_query() {
        let provider = WebSearchProvider::new(vec![WebSearchEngine {
            prefix: "g".to_owned(),
            name: "Google".to_owned(),
            url_template: "https://www.google.com/search?q={query}".to_owned(),
        }]);

        let results = provider.search(&QueryRequest {
            original: "g: tauri plugins".to_owned(),
            text: "tauri plugins".to_owned(),
            mode: SearchMode::WebSearch {
                prefix: "g".to_owned(),
            },
        });

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Google: tauri plugins");
        assert_eq!(results[0].provider, "web-search");
        assert_eq!(results[0].kind, SearchResultKind::WebSearch);
        assert_eq!(
            results[0].main_action,
            Action::OpenUrl {
                url: "https://www.google.com/search?q=tauri%20plugins".to_owned()
            }
        );
    }

    #[test]
    fn web_search_provider_does_not_auto_search_plain_queries() {
        let provider = WebSearchProvider::new(vec![WebSearchEngine {
            prefix: "g".to_owned(),
            name: "Google".to_owned(),
            url_template: "https://www.google.com/search?q={query}".to_owned(),
        }]);

        let results = provider.search(&QueryRequest::new("no local result", SearchMode::Normal));

        assert!(results.is_empty());
    }

    #[test]
    fn command_provider_returns_enable_prompt_when_command_execution_is_disabled() {
        let provider = CommandProvider::new(CommandProviderConfig {
            enabled: false,
            prefix: ">".to_owned(),
        });

        let results = provider.search(&QueryRequest {
            original: "> git status".to_owned(),
            text: "git status".to_owned(),
            mode: SearchMode::Command,
        });

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].provider, "commands");
        assert_eq!(results[0].kind, SearchResultKind::Feedback);
        assert!(results[0].title.contains("启用命令执行"));
    }

    #[test]
    fn command_provider_returns_confirmable_command_when_enabled() {
        let provider = CommandProvider::new(CommandProviderConfig {
            enabled: true,
            prefix: ">".to_owned(),
        });

        let results = provider.search(&QueryRequest {
            original: "> git status".to_owned(),
            text: "git status".to_owned(),
            mode: SearchMode::Command,
        });

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].provider, "commands");
        assert_eq!(results[0].kind, SearchResultKind::Command);
        assert_eq!(results[0].title, "git status");
        assert_eq!(
            results[0].main_action,
            Action::ExecuteCommand {
                command: "git status".to_owned(),
                requires_confirmation: true,
            }
        );
    }

    fn indexed_entry(path: &str, name: &str, kind: IndexedEntryKind) -> IndexedEntry {
        IndexedEntry {
            path: path.to_owned(),
            name: name.to_owned(),
            kind,
        }
    }
}
