## MODIFIED Requirements

### Requirement: Launcher Keyboard Navigation

The launcher SHALL separate result navigation from input history browsing.

#### Scenario: Arrow keys navigate results by default

- **WHEN** a non-empty query has search results
- **AND** the user presses Up or Down
- **THEN** QuickFox SHALL move selection in the result list
- **AND** SHALL NOT replace the input with history text

#### Scenario: Shift enters history mode

- **WHEN** recent input history exists
- **AND** the launcher input is focused
- **AND** the user presses Shift
- **THEN** QuickFox SHALL show a history list
- **AND** Up and Down SHALL browse history entries

#### Scenario: History mode confirms selected entry

- **WHEN** history mode is active
- **AND** the user presses Enter
- **THEN** QuickFox SHALL copy the selected history entry into the input
- **AND** exit history mode
- **AND** SHALL NOT execute the entry until Enter is pressed again in normal mode

#### Scenario: History mode exits without closing launcher

- **WHEN** history mode is active
- **AND** the user presses Escape
- **THEN** QuickFox SHALL hide the history list
- **AND** keep the launcher open

### Requirement: Launcher Icon

QuickFox SHALL use a recognizable pixel-style fox icon for the app and tray.

#### Scenario: Bundle and tray share the product icon

- **WHEN** QuickFox is built or launched
- **THEN** the bundle icon and tray icon SHALL come from the repository pixel fox icon assets
