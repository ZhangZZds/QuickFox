## Why

QuickFox is now usable enough for daily release, but the desktop shell still behaves like a rough prototype: the global wake key is hard-coded, launcher windows can remain visible after focus moves away, tray actions are not consistently toggle-like, and settings lacks inline guidance. On Windows in particular, QuickFox should feel like a tray-resident utility rather than a minimized app window.

## What Changes

- Add custom global wake key recording in settings, allowing users to capture an arbitrary key chord or the existing double-Shift sequence.
- Store the wake shortcut in QuickFox config and use it for global keyboard matching, while preserving `Shift+Shift` as the default.
- Hide the launcher when it loses focus, and keep the existing behavior where pressing the wake key again hides the focused launcher.
- Change the tray menu entry to "显示/隐藏 QuickFox" and route it through the same launcher toggle behavior.
- Make tray Settings reliable by showing/focusing an existing settings window or recreating it when it has been closed or destroyed.
- Treat Windows as a tray-resident background app: no normal minimized main window is required, and closing settings does not exit QuickFox.
- Remove the settings page "返回搜索" action; settings is an independent configuration window.
- Add small help icons beside settings fields, with hover/focus tooltips explaining how to configure each item.
- Improve settings page responsiveness so content adapts to window size without clipped controls or oversized fixed panels.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `launcher-shell`: custom wake-key recording, launcher auto-hide/toggle behavior, settings-window independence, settings help affordances, and responsive settings layout.
- `configuration-and-history`: persist and validate the global wake shortcut configuration.
- `actions-and-platform`: Windows tray-resident lifecycle, reliable tray settings window creation, and tray show/hide launcher routing.

## Impact

- Frontend: settings UI, shortcut recorder, help tooltip components, responsive CSS, and tests in `src/App.tsx`, `src/styles.css`, and `src/App.test.tsx`.
- Tauri client contract: commands/events for current shortcut capture, shortcut status, and launcher/settings window behavior in `src/tauriClient.ts`.
- Rust core: config model, hotkey matcher, global keyboard listener, tray menu routing, window focus/lifecycle handling, and tests in `src-tauri/src`.
- Manual QA: Windows tray-resident behavior, focus-loss hiding, custom shortcut recording, and reliable settings display.
