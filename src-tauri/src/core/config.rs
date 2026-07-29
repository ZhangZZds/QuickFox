//! Configuration loading will live here.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::core::platform::WakeShortcut;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickFoxConfig {
    pub index: IndexConfig,
    pub query: QueryConfig,
    pub web_search: WebSearchConfig,
    pub command: CommandConfig,
    pub history: HistoryConfig,
    pub results: ResultsConfig,
    #[serde(default)]
    pub hotkey: HotkeyConfig,
}

impl QuickFoxConfig {
    pub fn default_with_index_dirs(include_dirs: Vec<String>) -> Self {
        Self {
            index: IndexConfig {
                include_dirs,
                exclude_dirs: Vec::new(),
                exclude_patterns: vec![
                    "node_modules".to_owned(),
                    "target".to_owned(),
                    ".git".to_owned(),
                ],
                performance_mode: IndexPerformanceMode::Balanced,
                respect_project_ignores: true,
                content_include_dirs: default_content_include_dirs(),
                content_max_file_bytes: default_content_max_file_bytes(),
                watcher_enabled: true,
            },
            query: QueryConfig {
                regex_prefix: "re:".to_owned(),
            },
            web_search: WebSearchConfig {
                engines: BTreeMap::from([
                    (
                        "g".to_owned(),
                        WebSearchEngineConfig {
                            name: "Google".to_owned(),
                            url: "https://www.google.com/search?q={query}".to_owned(),
                        },
                    ),
                    (
                        "ddg".to_owned(),
                        WebSearchEngineConfig {
                            name: "DuckDuckGo".to_owned(),
                            url: "https://duckduckgo.com/?q={query}".to_owned(),
                        },
                    ),
                    (
                        "bd".to_owned(),
                        WebSearchEngineConfig {
                            name: "Baidu".to_owned(),
                            url: "https://www.baidu.com/s?wd={query}".to_owned(),
                        },
                    ),
                ]),
            },
            command: CommandConfig {
                prefix: ">".to_owned(),
                enabled: false,
            },
            history: HistoryConfig {
                input_history_enabled: true,
                input_max_entries: 15,
                file_history_enabled: true,
                calculator_history_enabled: false,
                web_search_history_enabled: false,
                command_history_enabled: true,
                command_max_entries: 15,
            },
            results: ResultsConfig { limit: 20 },
            hotkey: HotkeyConfig {
                wake_shortcut: default_wake_shortcut(),
            },
        }
    }

    pub fn validate(&self) -> Vec<ConfigValidationError> {
        let mut errors: Vec<_> = self
            .web_search
            .engines
            .iter()
            .filter_map(|(prefix, engine)| {
                if engine.url.contains("{query}") {
                    None
                } else {
                    Some(ConfigValidationError::WebSearchTemplateMissingQuery {
                        prefix: prefix.clone(),
                    })
                }
            })
            .collect();
        if WakeShortcut::parse(&self.hotkey.wake_shortcut).is_none() {
            errors.push(ConfigValidationError::InvalidWakeShortcut {
                value: self.hotkey.wake_shortcut.clone(),
            });
        }
        errors
    }

    pub fn valid_web_search_engines(&self) -> BTreeMap<String, WebSearchEngineConfig> {
        self.web_search
            .engines
            .iter()
            .filter(|(_, engine)| engine.url.contains("{query}"))
            .map(|(prefix, engine)| (prefix.clone(), engine.clone()))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexConfig {
    pub include_dirs: Vec<String>,
    pub exclude_dirs: Vec<String>,
    pub exclude_patterns: Vec<String>,
    #[serde(default)]
    pub performance_mode: IndexPerformanceMode,
    #[serde(default = "default_true")]
    pub respect_project_ignores: bool,
    #[serde(default = "default_content_include_dirs")]
    pub content_include_dirs: Vec<String>,
    #[serde(default = "default_content_max_file_bytes")]
    pub content_max_file_bytes: u64,
    #[serde(default = "default_true")]
    pub watcher_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IndexPerformanceMode {
    Fast,
    #[default]
    Balanced,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryConfig {
    pub regex_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchConfig {
    pub engines: BTreeMap<String, WebSearchEngineConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchEngineConfig {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandConfig {
    pub prefix: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryConfig {
    #[serde(default = "default_true")]
    pub input_history_enabled: bool,
    #[serde(default = "default_history_entries")]
    pub input_max_entries: usize,
    #[serde(default = "default_true")]
    pub file_history_enabled: bool,
    #[serde(default)]
    pub calculator_history_enabled: bool,
    #[serde(default)]
    pub web_search_history_enabled: bool,
    #[serde(default = "default_true")]
    pub command_history_enabled: bool,
    #[serde(default = "default_history_entries")]
    pub command_max_entries: usize,
}

fn default_true() -> bool {
    true
}

fn default_history_entries() -> usize {
    15
}

fn default_content_max_file_bytes() -> u64 {
    2 * 1024 * 1024
}

fn default_content_include_dirs() -> Vec<String> {
    let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
    else {
        return Vec::new();
    };

    #[cfg(target_os = "windows")]
    {
        return ["Desktop", "桌面"]
            .into_iter()
            .map(|name| home.join(name))
            .filter(|path| path.is_dir())
            .map(path_to_config_string)
            .collect();
    }

    #[cfg(not(target_os = "windows"))]
    {
        ["Desktop", "Documents", "Downloads", "workspace"]
            .into_iter()
            .map(|name| home.join(name))
            .filter(|path| path.is_dir())
            .map(path_to_config_string)
            .collect()
    }
}

fn path_to_config_string(path: PathBuf) -> String {
    path.to_string_lossy().to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultsConfig {
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyConfig {
    #[serde(default = "default_wake_shortcut")]
    pub wake_shortcut: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            wake_shortcut: default_wake_shortcut(),
        }
    }
}

fn default_wake_shortcut() -> String {
    "Shift+Shift".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigValidationError {
    WebSearchTemplateMissingQuery { prefix: String },
    InvalidWakeShortcut { value: String },
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    ParseToml(toml::de::Error),
    SerializeToml(toml::ser::Error),
}

impl From<std::io::Error> for ConfigError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(error: toml::de::Error) -> Self {
        Self::ParseToml(error)
    }
}

impl From<toml::ser::Error> for ConfigError {
    fn from(error: toml::ser::Error) -> Self {
        Self::SerializeToml(error)
    }
}

#[derive(Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn save(&self, config: &QuickFoxConfig) -> Result<(), ConfigError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(config)?;
        fs::write(&self.path, content)?;
        Ok(())
    }

    pub fn load(&self) -> Result<QuickFoxConfig, ConfigError> {
        let content = fs::read_to_string(&self.path)?;
        Ok(toml::from_str(&content)?)
    }

    pub fn load_or_create_default(
        &self,
        default_index_dirs: Vec<String>,
    ) -> Result<QuickFoxConfig, ConfigError> {
        if self.path.exists() {
            return self.load();
        }

        let config = QuickFoxConfig::default_with_index_dirs(default_index_dirs);
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&config)?;
        fs::write(&self.path, content)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn load_or_create_default_writes_toml_when_config_is_missing() {
        let path = temp_config_path("default");
        let store = ConfigStore::new(path.clone());

        let config = store
            .load_or_create_default(vec!["/home/frank".to_owned()])
            .unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(config.index.include_dirs, vec!["/home/frank"]);
        assert_eq!(config.query.regex_prefix, "re:");
        assert!(!config.command.enabled);
        assert_eq!(config.command.prefix, ">");
        assert_eq!(config.history.command_max_entries, 15);
        assert_eq!(config.history.input_max_entries, 15);
        assert_eq!(config.hotkey.wake_shortcut, "Shift+Shift");
        assert!(config.history.input_history_enabled);
        assert!(content.contains("regex_prefix = \"re:\""));
        assert!(content.contains("enabled = false"));
        assert!(content.contains("wake_shortcut = \"Shift+Shift\""));
        assert_eq!(
            config.web_search.engines.get("ddg").unwrap().url,
            "https://duckduckgo.com/?q={query}"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn default_index_config_enables_balanced_local_content_indexing() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/home/frank".to_owned()]);

        assert_eq!(
            config.index.performance_mode,
            IndexPerformanceMode::Balanced
        );
        assert!(config.index.respect_project_ignores);
        assert_eq!(config.index.content_max_file_bytes, 2 * 1024 * 1024);
        assert!(config.index.watcher_enabled);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_default_content_index_dirs_only_include_desktop() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["C:\\Users\\frank".to_owned()]);

        assert!(config
            .index
            .content_include_dirs
            .iter()
            .all(|path| path.ends_with("\\Desktop") || path.ends_with("\\桌面")));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_default_content_index_dirs_include_existing_hot_paths() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/Users/frank".to_owned()]);

        assert!(config.index.content_include_dirs.iter().all(|path| {
            path.ends_with("/Desktop")
                || path.ends_with("/Documents")
                || path.ends_with("/Downloads")
                || path.ends_with("/workspace")
        }));
    }

    #[test]
    fn existing_config_without_new_index_fields_uses_defaults() {
        let config: QuickFoxConfig = toml::from_str(
            r#"
[index]
include_dirs = ["/home/frank"]
exclude_dirs = []
exclude_patterns = []

[query]
regex_prefix = "re:"

[web_search.engines.g]
name = "Google"
url = "https://www.google.com/search?q={query}"

[command]
prefix = ">"
enabled = false

[history]
input_history_enabled = true
input_max_entries = 15
file_history_enabled = true
calculator_history_enabled = false
web_search_history_enabled = false
command_history_enabled = true
command_max_entries = 15

[results]
limit = 20
"#,
        )
        .unwrap();

        assert_eq!(
            config.index.performance_mode,
            IndexPerformanceMode::Balanced
        );
        assert!(config.index.respect_project_ignores);
        assert_eq!(config.index.content_max_file_bytes, 2 * 1024 * 1024);
        assert!(config.index.watcher_enabled);
    }

    #[test]
    fn load_reads_modified_regex_prefix_and_web_search_engines() {
        let path = temp_config_path("load");
        fs::write(
            &path,
            r#"
[index]
include_dirs = ["/home/frank"]
exclude_dirs = []
exclude_patterns = []

[query]
regex_prefix = "regex:"

[web_search.engines.g]
name = "Google"
url = "https://www.google.com/search?q={query}"

[command]
prefix = ">"
enabled = false

[history]
input_history_enabled = true
input_max_entries = 15
file_history_enabled = true
calculator_history_enabled = false
web_search_history_enabled = false
command_history_enabled = true
command_max_entries = 15

[results]
limit = 20

[hotkey]
wake_shortcut = "Control+Space"
"#,
        )
        .unwrap();
        let store = ConfigStore::new(path.clone());

        let config = store.load().unwrap();

        assert_eq!(config.query.regex_prefix, "regex:");
        assert_eq!(config.hotkey.wake_shortcut, "Control+Space");
        assert_eq!(config.web_search.engines["g"].name, "Google");
        assert_eq!(
            config.web_search.engines["g"].url,
            "https://www.google.com/search?q={query}"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn save_persists_updated_command_settings() {
        let path = temp_config_path("save");
        let store = ConfigStore::new(path.clone());
        let mut config = QuickFoxConfig::default_with_index_dirs(vec!["/Users/frank".to_owned()]);
        config.command.enabled = true;
        config.history.input_max_entries = 30;
        config.history.command_max_entries = 30;
        config.hotkey.wake_shortcut = "Command+Shift+K".to_owned();

        store.save(&config).unwrap();

        let loaded = store.load().unwrap();
        assert!(loaded.command.enabled);
        assert_eq!(loaded.history.input_max_entries, 30);
        assert_eq!(loaded.history.command_max_entries, 30);
        assert_eq!(loaded.hotkey.wake_shortcut, "Command+Shift+K");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn validate_reports_web_search_templates_missing_query_placeholder() {
        let mut config = QuickFoxConfig::default_with_index_dirs(vec!["/home/frank".to_owned()]);
        config.web_search.engines.insert(
            "bad".to_owned(),
            WebSearchEngineConfig {
                name: "Broken".to_owned(),
                url: "https://example.com/search".to_owned(),
            },
        );

        let errors = config.validate();

        assert_eq!(
            errors,
            vec![ConfigValidationError::WebSearchTemplateMissingQuery {
                prefix: "bad".to_owned()
            }]
        );
        assert!(!config.valid_web_search_engines().contains_key("bad"));
    }

    #[test]
    fn load_defaults_wake_shortcut_for_existing_config_without_hotkey_section() {
        let path = temp_config_path("load-default-hotkey");
        fs::write(
            &path,
            r#"
[index]
include_dirs = ["/home/frank"]
exclude_dirs = []
exclude_patterns = []

[query]
regex_prefix = "regex:"

[web_search.engines.g]
name = "Google"
url = "https://www.google.com/search?q={query}"

[command]
prefix = ">"
enabled = false

[history]
input_history_enabled = true
input_max_entries = 15
file_history_enabled = true
calculator_history_enabled = false
web_search_history_enabled = false
command_history_enabled = true
command_max_entries = 15

[results]
limit = 20
"#,
        )
        .unwrap();
        let store = ConfigStore::new(path.clone());

        let config = store.load().unwrap();

        assert_eq!(config.hotkey.wake_shortcut, "Shift+Shift");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn validate_rejects_invalid_wake_shortcut() {
        let mut config = QuickFoxConfig::default_with_index_dirs(vec!["/home/frank".to_owned()]);
        config.hotkey.wake_shortcut = "Shift".to_owned();

        assert!(config
            .validate()
            .contains(&ConfigValidationError::InvalidWakeShortcut {
                value: "Shift".to_owned()
            }));
    }

    fn temp_config_path(label: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("quickfox-{label}-{suffix}.toml"))
    }
}
