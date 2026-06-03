## ADDED Requirements

### Requirement: Open With Development Tool

QuickFox SHALL provide a secondary action to open file and directory results with a development tool.

#### Scenario: File result exposes development open action

- **WHEN** a file search result is returned
- **THEN** the result SHALL include a secondary action labelled as development open in the UI
- **AND** the action SHALL be executed by Rust core through a platform adapter

#### Scenario: Development open adapter chooses available tool

- **WHEN** the user invokes development open
- **THEN** QuickFox SHALL prefer installed code editors such as VS Code or Cursor
- **AND** SHALL use platform-specific fallbacks without Provider-specific shell logic
