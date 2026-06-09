//! Structured file query parser boundary.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileQuery {
    pub ordinary_terms: Vec<String>,
    pub type_filters: Vec<String>,
    pub name_filters: Vec<String>,
    pub dir_filters: Vec<String>,
    pub content_queries: Vec<String>,
}

impl FileQuery {
    pub fn parse(input: &str) -> Self {
        let mut query = Self::default();
        let tokens = tokenize(input);
        let mut index = 0;

        while let Some(token) = tokens.get(index).cloned() {
            index += 1;
            let Some((field, value)) = token.split_once(':') else {
                push_non_empty(&mut query.ordinary_terms, token);
                continue;
            };

            let field = field.to_ascii_lowercase();
            if value.is_empty() {
                if is_known_field(&field) {
                    if let Some(next_value) = tokens.get(index).cloned() {
                        index += 1;
                        push_field_value(&mut query, &field, next_value);
                        continue;
                    }
                }
                push_non_empty(&mut query.ordinary_terms, token);
                continue;
            }

            if is_known_field(&field) {
                push_field_value(&mut query, &field, value.to_owned());
            } else {
                push_non_empty(&mut query.ordinary_terms, token);
            }
        }

        query
    }

    pub fn has_content_query(&self) -> bool {
        !self.content_queries.is_empty()
    }

    pub fn has_name_path_constraints(&self) -> bool {
        !self.ordinary_terms.is_empty()
            || !self.type_filters.is_empty()
            || !self.name_filters.is_empty()
            || !self.dir_filters.is_empty()
    }
}

fn is_known_field(field: &str) -> bool {
    matches!(field, "type" | "name" | "dir" | "content")
}

fn push_field_value(query: &mut FileQuery, field: &str, value: String) {
    match field {
        "type" => push_non_empty(&mut query.type_filters, normalize_extension(&value)),
        "name" => push_non_empty(&mut query.name_filters, value),
        "dir" => push_non_empty(&mut query.dir_filters, value),
        "content" => push_non_empty(&mut query.content_queries, value),
        _ => {}
    }
}

fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut in_quotes = false;
    let mut chars = input.trim().chars().peekable();

    while let Some(character) = chars.next() {
        match character {
            '"' | '“' | '”' => {
                in_quotes = !in_quotes;
            }
            character if character.is_whitespace() && !in_quotes => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
                while chars.peek().is_some_and(|next| next.is_whitespace()) {
                    chars.next();
                }
            }
            _ => token.push(character),
        }
    }

    if !token.is_empty() {
        tokens.push(token);
    }

    tokens
}

fn normalize_extension(value: &str) -> String {
    trim_wrapping_quotes(value)
        .trim_start_matches('.')
        .to_ascii_lowercase()
}

fn push_non_empty(values: &mut Vec<String>, value: String) {
    let value = trim_wrapping_quotes(&value);
    if !value.is_empty() {
        values.push(value.to_owned());
    }
}

fn trim_wrapping_quotes(value: &str) -> &str {
    value.trim_matches(|character| matches!(character, '"' | '“' | '”'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ordinary_terms() {
        let query = FileQuery::parse("report budget");

        assert_eq!(query.ordinary_terms, vec!["report", "budget"]);
        assert!(query.type_filters.is_empty());
        assert!(query.name_filters.is_empty());
        assert!(query.dir_filters.is_empty());
        assert!(query.content_queries.is_empty());
        assert!(!query.has_content_query());
    }

    #[test]
    fn parses_field_filters() {
        assert_eq!(FileQuery::parse("type:pdf").type_filters, vec!["pdf"]);
        assert_eq!(FileQuery::parse("type:.PDF").type_filters, vec!["pdf"]);
        assert_eq!(FileQuery::parse("name:test").name_filters, vec!["test"]);
        assert_eq!(
            FileQuery::parse("dir:workspace").dir_filters,
            vec!["workspace"]
        );
        assert_eq!(
            FileQuery::parse("dir:**/workspace").dir_filters,
            vec!["**/workspace"]
        );
    }

    #[test]
    fn parses_field_values_after_space_following_colon() {
        let query = FileQuery::parse("Agent type: md");

        assert_eq!(query.ordinary_terms, vec!["Agent"]);
        assert_eq!(query.type_filters, vec!["md"]);
    }

    #[test]
    fn parses_quoted_field_values_and_windows_paths() {
        let query = FileQuery::parse(r#"name:"project report" dir:"D:\My Projects""#);

        assert_eq!(query.name_filters, vec!["project report"]);
        assert_eq!(query.dir_filters, vec![r#"D:\My Projects"#]);
        assert!(query.ordinary_terms.is_empty());
    }

    #[test]
    fn parses_content_without_treating_it_as_ordinary_text() {
        let query = FileQuery::parse(r#"workspace type:md content:"hello world""#);

        assert_eq!(query.ordinary_terms, vec!["workspace"]);
        assert_eq!(query.type_filters, vec!["md"]);
        assert_eq!(query.content_queries, vec!["hello world"]);
        assert!(query.has_content_query());
    }

    #[test]
    fn parses_content_with_smart_quotes() {
        let query = FileQuery::parse("name:Agent content:”openspec”");

        assert_eq!(query.name_filters, vec!["Agent"]);
        assert_eq!(query.content_queries, vec!["openspec"]);
        assert!(query.ordinary_terms.is_empty());
    }
}
