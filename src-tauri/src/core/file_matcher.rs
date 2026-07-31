//! File name/path matcher boundary.

use crate::core::file_query::FileQuery;
use crate::core::index_entry::{build_search_text, IndexedEntry};
use globset::{Glob, GlobSet};
use nucleo_matcher::{Matcher, Utf32Str};

#[derive(Debug, Clone, Default)]
pub struct FileMatcher {
    name_path: StableNamePathMatcher,
}

#[derive(Debug, Clone, Copy)]
pub struct FileMatchCandidate<'a> {
    pub path: &'a str,
    pub name: &'a str,
    pub parent: &'a str,
    pub extension: Option<&'a str>,
    pub search_text: &'a str,
    pub has_custom_search_text: bool,
}

impl FileMatcher {
    pub fn matches(&self, query: &FileQuery, entry: &IndexedEntry) -> bool {
        let search_text = entry_search_text(entry);
        self.matches_with_search_text(query, entry, &search_text)
    }

    pub fn matches_with_search_text(
        &self,
        query: &FileQuery,
        entry: &IndexedEntry,
        search_text: &str,
    ) -> bool {
        let standard_search_text = build_search_text(&entry.name, &entry.path);
        self.matches_candidate(
            query,
            FileMatchCandidate {
                path: &entry.path,
                name: &entry.name,
                parent: &entry.parent,
                extension: entry.extension.as_deref(),
                search_text,
                has_custom_search_text: !entry.search_text.is_empty()
                    && search_text != standard_search_text,
            },
        )
    }

    pub fn matches_candidate(&self, query: &FileQuery, entry: FileMatchCandidate<'_>) -> bool {
        query.type_filters.iter().all(|expected| {
            entry_extension(entry.name, entry.extension)
                .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
        }) && query.name_filters.iter().all(|filter| {
            entry
                .name
                .to_ascii_lowercase()
                .contains(&filter.to_ascii_lowercase())
        }) && query
            .dir_filters
            .iter()
            .all(|filter| dir_filter_matches(filter, entry.path, entry.parent))
            && query
                .ordinary_terms
                .iter()
                .all(|term| matches_name_path_term(&self.name_path, term, entry))
    }
}

fn matches_name_path_term(
    matcher: &StableNamePathMatcher,
    term: &str,
    entry: FileMatchCandidate<'_>,
) -> bool {
    matcher.matches_term_fields(
        term,
        entry.name,
        entry.path,
        entry.search_text,
        entry.has_custom_search_text,
    )
}

pub trait NamePathMatcher {
    fn matches_term(&self, term: &str, entry: &IndexedEntry, search_text: &str) -> bool;
}

#[derive(Debug, Clone, Default)]
pub struct StableNamePathMatcher;

impl NamePathMatcher for StableNamePathMatcher {
    fn matches_term(&self, term: &str, entry: &IndexedEntry, search_text: &str) -> bool {
        self.matches_term_fields(
            term,
            &entry.name,
            &entry.path,
            search_text,
            uses_custom_cached_text(entry, search_text),
        )
    }
}

impl StableNamePathMatcher {
    fn matches_term_fields(
        &self,
        term: &str,
        name: &str,
        path: &str,
        search_text: &str,
        has_custom_search_text: bool,
    ) -> bool {
        let term = term.trim().to_ascii_lowercase();
        if term.is_empty() {
            return true;
        }

        if has_custom_search_text && search_text.contains(&term) {
            return true;
        }

        let name = name.to_ascii_lowercase();
        if name.contains(&term) {
            return true;
        }

        if name_segments(&name).any(|segment| {
            segment.starts_with(&term)
                || segment.chars().next().zip(term.chars().next()).is_some_and(
                    |(segment_first, term_first)| {
                        segment_first == term_first && fuzzy_segment_matches(&term, &segment)
                    },
                )
        }) {
            return true;
        }

        if term.contains(['/', '\\']) {
            return search_text.to_lowercase().contains(&term.to_lowercase());
        }

        path_segments(path).any(|segment| {
            segment.starts_with(&term)
                || segment.chars().next().zip(term.chars().next()).is_some_and(
                    |(segment_first, term_first)| {
                        segment_first == term_first && fuzzy_segment_matches(&term, &segment)
                    },
                )
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct NucleoNamePathMatcher;

impl NucleoNamePathMatcher {
    pub fn score(&self, needle: &str, haystack: &str) -> Option<u16> {
        let needle = needle.to_ascii_lowercase();
        let haystack = haystack.to_ascii_lowercase();
        let mut needle_buf = Vec::new();
        let mut haystack_buf = Vec::new();
        let needle = Utf32Str::new(&needle, &mut needle_buf);
        let haystack = Utf32Str::new(&haystack, &mut haystack_buf);
        Matcher::new(nucleo_matcher::Config::DEFAULT).fuzzy_match(haystack, needle)
    }
}

fn entry_search_text(entry: &IndexedEntry) -> String {
    if entry.search_text.is_empty() {
        build_search_text(&entry.name, &entry.path)
    } else {
        entry.search_text.clone()
    }
}

fn uses_custom_cached_text(entry: &IndexedEntry, search_text: &str) -> bool {
    !entry.search_text.is_empty() && search_text != build_search_text(&entry.name, &entry.path)
}

fn entry_extension<'a>(name: &'a str, extension: Option<&'a str>) -> Option<&'a str> {
    extension
        .map(|extension| extension.trim_start_matches('.'))
        .or_else(|| name.rsplit_once('.').map(|(_, extension)| extension))
}

fn dir_filter_matches(filter: &str, path: &str, parent: &str) -> bool {
    let directory = entry_directory(path, parent);
    if has_glob_meta(filter) {
        return glob_matches(filter, &directory);
    }

    directory
        .to_ascii_lowercase()
        .contains(&filter.to_ascii_lowercase())
}

fn entry_directory(path: &str, parent: &str) -> String {
    if !parent.is_empty() {
        return normalize_path(parent);
    }

    path.rsplit_once(['/', '\\'])
        .map(|(parent, _)| normalize_path(parent))
        .unwrap_or_default()
}

fn glob_matches(pattern: &str, directory: &str) -> bool {
    let pattern = normalize_path(pattern);
    let directory = normalize_path(directory);
    let Ok(glob) = Glob::new(&pattern) else {
        return false;
    };

    let mut builder = globset::GlobSetBuilder::new();
    builder.add(glob);
    let Ok(set): Result<GlobSet, _> = builder.build() else {
        return false;
    };

    set.is_match(&directory)
}

fn has_glob_meta(value: &str) -> bool {
    value.contains(['*', '?', '['])
}

fn normalize_path(value: &str) -> String {
    value.replace('\\', "/")
}

fn name_segments(name: &str) -> impl Iterator<Item = String> + '_ {
    name.split(|candidate: char| !candidate.is_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
}

fn path_segments(path: &str) -> impl Iterator<Item = String> + '_ {
    path.split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_ascii_lowercase())
}

fn fuzzy_segment_matches(query: &str, segment: &str) -> bool {
    let mut chars = query.chars();
    let Some(mut current) = chars.next() else {
        return true;
    };
    let mut first_match: Option<usize> = None;

    for (index, candidate) in segment.chars().enumerate() {
        if candidate == current {
            first_match.get_or_insert(index);
            match chars.next() {
                Some(next) => current = next,
                None => {
                    let span = index.saturating_sub(first_match.unwrap_or(index)) + 1;
                    let max_span = query.chars().count().saturating_mul(2).max(8);
                    return span <= max_span;
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index_entry::{IndexedEntry, IndexedEntryKind};

    #[test]
    fn field_filters_use_and_semantics() {
        let matcher = FileMatcher::default();
        let entry = entry("/Users/frank/workspace/reports/budget.PDF");
        let query = FileQuery::parse("budget type:pdf name:budget dir:workspace");

        assert!(matcher.matches(&query, &entry));
        assert!(!matcher.matches(&FileQuery::parse("budget type:md"), &entry));
        assert!(!matcher.matches(&FileQuery::parse("budget name:workspace"), &entry));
        assert!(!matcher.matches(&FileQuery::parse("budget dir:downloads"), &entry));
    }

    #[test]
    fn name_filter_matches_file_name_not_parent_only() {
        let matcher = FileMatcher::default();
        let entry = entry("/Users/frank/test-fixtures/report.md");

        assert!(matcher.matches(&FileQuery::parse("name:report"), &entry));
        assert!(!matcher.matches(&FileQuery::parse("name:test-fixtures"), &entry));
    }

    #[test]
    fn dir_filter_supports_contains_and_glob() {
        let matcher = FileMatcher::default();
        let entry = entry("/Users/frank/workspace/QuickFox/README.md");

        assert!(matcher.matches(&FileQuery::parse("dir:workspace"), &entry));
        assert!(matcher.matches(&FileQuery::parse("dir:**/workspace/QuickFox"), &entry));
        assert!(!matcher.matches(&FileQuery::parse("dir:**/Downloads"), &entry));
    }

    #[test]
    fn ordinary_terms_use_precomputed_search_text() {
        let matcher = FileMatcher::default();
        let mut entry = entry("/unrelated/path.bin");
        entry.search_text = "synthetic cached needle".to_owned();

        assert!(matcher.matches(&FileQuery::parse("needle"), &entry));
    }

    fn entry(path: &str) -> IndexedEntry {
        IndexedEntry::legacy(
            path,
            path.rsplit(['/', '\\']).next().unwrap().to_owned(),
            IndexedEntryKind::File,
        )
    }
}
