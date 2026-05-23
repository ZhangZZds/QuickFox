//! Configuration loading will live here.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickFoxConfig {
    pub index: IndexConfig,
    pub query: QueryConfig,
    pub web_search: WebSearchConfig,
    pub command: CommandConfig,
    pub history: HistoryConfig,
    pub results: ResultsConfig,
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
                        "gh".to_owned(),
                        WebSearchEngineConfig {
                            name: "GitHub".to_owned(),
                            url: "https://github.com/search?q={query}".to_owned(),
                        },
                    ),
                ]),
            },
            command: CommandConfig {
                prefix: ">".to_owned(),
                enabled: false,
            },
            history: HistoryConfig {
                file_history_enabled: true,
                calculator_history_enabled: false,
                web_search_history_enabled: false,
                command_history_enabled: true,
                command_max_entries: 15,
            },
            results: ResultsConfig { limit: 20 },
        }
    }

    pub fn validate(&self) -> Vec<ConfigValidationError> {
        self.web_search
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
            .collect()
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
    pub file_history_enabled: bool,
    pub calculator_history_enabled: bool,
    pub web_search_history_enabled: bool,
    pub command_history_enabled: bool,
    pub command_max_entries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultsConfig {
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigValidationError {
    WebSearchTemplateMissingQuery { prefix: String },
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
        assert!(content.contains("regex_prefix = \"re:\""));
        assert!(content.contains("enabled = false"));

        let _ = fs::remove_file(path);
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

        assert_eq!(config.query.regex_prefix, "regex:");
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
        config.history.command_max_entries = 30;

        store.save(&config).unwrap();

        let loaded = store.load().unwrap();
        assert!(loaded.command.enabled);
        assert_eq!(loaded.history.command_max_entries, 30);

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

    fn temp_config_path(label: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("quickfox-{label}-{suffix}.toml"))
    }
}
