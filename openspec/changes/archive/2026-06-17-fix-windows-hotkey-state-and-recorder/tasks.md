## Implementation Tasks

- [x] 1. Rust hotkey state machine
  - [x] 1.1 Add failing tests for `Shift+Space` stale modifier state, timeout, and repeated wake key behavior
  - [x] 1.2 Update `HotkeyState` chord handling to require a fresh modifier press within the valid window
  - [x] 1.3 Verify default `Shift+Shift` behavior remains unchanged
- [x] 2. Frontend shortcut recorder
  - [x] 2.1 Add failing test for `Alt+Space` recording rejection
  - [x] 2.2 Reject known reserved/high-risk global wake shortcuts with actionable Chinese feedback
  - [x] 2.3 Verify existing `Control+Space`, `Command+Shift+K`, Esc cancel, and `Shift+Shift` recording behavior remains unchanged
- [x] 3. Proactive exploration and documentation
  - [x] 3.1 Update Windows manual QA checklist for `Shift+Space` typing and `Alt+Space` recording
  - [x] 3.2 Document the recommended autonomous exploration approach: pure state-machine event fuzz/table tests, frontend keyboard simulation, and Windows desktop manual/UI automation follow-up
- [x] 4. Verification
  - [x] 4.1 Run targeted Rust tests for hotkey state
  - [x] 4.2 Run targeted frontend tests for shortcut recording
  - [x] 4.3 Run `openspec validate fix-windows-hotkey-state-and-recorder --strict`
