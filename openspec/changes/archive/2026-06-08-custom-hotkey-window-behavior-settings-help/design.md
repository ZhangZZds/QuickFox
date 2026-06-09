## Context

QuickFox currently hard-codes `Shift+Shift` in the low-level `keytap` listener. Window behavior is split between Tauri window APIs and a small `LauncherWindowState`, while settings is rendered in a separate Tauri window. The settings UI already has sections, but it lacks inline guidance, still exposes a "返回搜索" action, and uses several fixed dimensions that make resizing brittle.

The requested change spans Rust config, global keyboard event matching, Tauri tray/window behavior, and frontend settings UX. It must preserve the tray-resident launcher model: QuickFox keeps running in the background, the launcher is a temporary floating surface, and settings is an independent configuration window.

## Goals / Non-Goals

**Goals:**

- Let users record a custom global wake shortcut from settings, while keeping double-Shift as the default.
- Make wake shortcut matching configurable and testable without relying on live OS keyboard hooks in unit tests.
- Hide the launcher when focus leaves it, and toggle the launcher from both the wake shortcut and tray menu.
- Make settings opening reliable even if the settings window was closed/destroyed.
- Remove settings-to-search navigation from the settings window.
- Add hover/focus help icons for configuration fields.
- Make settings layout respond to window size instead of depending on a fixed 920x640 surface.
- Keep Windows behavior tray-resident: no normal minimized main window is required for background operation.

**Non-Goals:**

- A complete system-wide shortcut conflict detector. The app will explain that OS or third-party conflicts can prevent delivery.
- A plugin API for arbitrary shortcut actions.
- Runtime restart of the low-level keyboard hook thread without a config save boundary if the platform hook cannot be safely reconfigured. Re-reading the saved shortcut on each event is acceptable for this scope.
- Replacing `keytap` with Tauri's accelerator-only global shortcut plugin; double-key sequences still need low-level event matching.

## Decisions

### Shortcut Data Model

Store wake shortcut config under a new `hotkey` config section:

- `wake_shortcut`: a normalized string such as `Shift+Shift`, `Control+Space`, `Command+Shift+K`, or `Alt+Space`.

Rust parses this into a `WakeShortcut` enum:

- `DoubleShift` for the existing double-Shift sequence.
- `Chord { modifiers, key }` for one non-modifier key plus zero or more modifiers.

This keeps persisted config human-readable and avoids committing to a frontend-only event shape. Invalid or empty saved values fall back to `Shift+Shift` and surface a status message.

### Recording Flow

The frontend records shortcut candidates using normal browser/Tauri keydown events while the settings window has focus. It normalizes platform labels for display and submits the normalized string through `saveConfig`. Recording is not the same as registering; it is just a controlled way to edit config. Rust remains the source of truth by validating and matching the stored string.

The recorder accepts:

- Double press of Shift within the recorder timeout as `Shift+Shift`.
- A chord when a non-modifier key is pressed with optional modifiers.

The recorder rejects bare single modifier keys except the double-Shift sequence, because they create too many accidental activations.

### Runtime Matching

The low-level `keytap` listener maps events into a platform-neutral `KeyPress` containing modifiers and a key identity. `HotkeyState` becomes configurable: it receives a `WakeShortcut` and determines whether the current event completes the shortcut. For now the listener may read the current config from app state per key event; this is simple, avoids cross-thread re-registration, and makes saves take effect without restarting the app.

### Window Behavior

The launcher has one behavior contract:

- hidden or backgrounded + wake/toggle -> show and focus
- focused + wake/toggle -> hide
- focus lost -> hide

Tauri window focus events will update `LauncherWindowState` and hide the launcher when the main launcher window loses focus. Settings windows are excluded from this auto-hide behavior.

The tray menu item becomes "显示/隐藏 QuickFox" and calls the same toggle path as the wake shortcut. This removes the current mismatch where tray show always forces the launcher visible.

### Settings Window Reliability

Settings opening uses an idempotent helper:

1. If `settings` window exists, show, unminimize, and focus it.
2. If it does not exist, create a new WebviewWindow labeled `settings` with the settings URL and window shape.
3. Do not show the launcher while opening settings.

Closing settings should close/hide only that window; QuickFox keeps running. The launcher remains hidden unless explicitly toggled.

### Settings UI

The settings window removes "返回搜索". The title bar contains "设置" and a save action, while platform window controls remain responsible for closing the settings window.

Help icons are small `?` buttons beside field labels. They expose explanatory text through hover and keyboard focus using accessible tooltip semantics. They must not be visible paragraphs in the normal layout.

Settings responsiveness uses CSS grid with `minmax`, `clamp`, and section-specific single-column fallbacks. The settings panel should fill available window space instead of clamping to a fixed size inside a resizable system window.

## Risks / Trade-offs

- **Platform key naming differences** → Normalize key names in one Rust parser/matcher and one frontend recorder helper, with tests for common aliases.
- **OS-reserved shortcuts cannot be intercepted** → Show help text near the recorder explaining that OS or app conflicts may prevent activation.
- **Reading config on every global key event adds lock traffic** → The config is small and key events are low-volume enough for this release; optimize later only if profiling shows a problem.
- **Settings window recreation can duplicate windows if labels drift** → Use a single `settings` label and tests around window routing helpers/config.
- **Focus-loss hiding can be surprising while interacting with context menus** → Scope the behavior to launcher window focus loss; settings is unaffected and launcher can always be recalled from tray/hotkey.
