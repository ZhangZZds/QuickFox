## MODIFIED Requirements

### Requirement: 网页搜索 Provider

Configured web search prefixes SHALL be executable from the launcher.

#### Scenario: Baidu prefix executes on Enter

- **WHEN** web search engine `bd` is configured
- **AND** the user types `bd 1234`
- **AND** presses Enter
- **THEN** QuickFox SHALL open `https://www.baidu.com/s?wd=1234`
- **AND** record `bd 1234` in input history

#### Scenario: Web search execution does not depend on rendered results

- **WHEN** the user enters a configured web search prefix query
- **AND** search results have not finished rendering
- **AND** the user presses Enter
- **THEN** QuickFox SHALL still execute the configured web search URL
