//! Query providers will live here.

use crate::core::search::{QueryRequest, SearchResult};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::actions::Action;
    use crate::core::search::{QueryRequest, SearchMode, SearchResult, SearchResultKind};

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
}
