//! Platform adapters will live here.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    Adapter(String),
    NoTerminalAvailable,
    CommandBlocked(String),
}

pub type PlatformResult<T = ()> = Result<T, PlatformError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKind {
    Windows,
    Linux,
    Macos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathAdapter {
    platform: PlatformKind,
    home_dir: Option<String>,
    user_profile: Option<String>,
    available_drive_roots: Vec<String>,
}

impl PathAdapter {
    pub fn new(platform: PlatformKind) -> Self {
        Self {
            platform,
            home_dir: None,
            user_profile: None,
            available_drive_roots: Vec::new(),
        }
    }

    pub fn with_home_dir(mut self, home_dir: impl Into<String>) -> Self {
        self.home_dir = Some(home_dir.into());
        self
    }

    pub fn with_user_profile(mut self, user_profile: impl Into<String>) -> Self {
        self.user_profile = Some(user_profile.into());
        self
    }

    pub fn with_available_drive_roots(mut self, roots: Vec<String>) -> Self {
        self.available_drive_roots = roots;
        self
    }

    pub fn default_index_roots(&self) -> Vec<String> {
        match self.platform {
            PlatformKind::Windows if !self.available_drive_roots.is_empty() => {
                self.available_drive_roots.clone()
            }
            PlatformKind::Windows => self
                .user_profile
                .clone()
                .or_else(|| self.home_dir.clone())
                .into_iter()
                .collect(),
            PlatformKind::Linux | PlatformKind::Macos => {
                self.home_dir.clone().into_iter().collect()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenTarget {
    File(String),
    Directory(String),
    ContainingFolder(String),
    Url(String),
}

pub trait OpenAdapter {
    fn open_file(&mut self, path: &str) -> PlatformResult;
    fn open_directory(&mut self, path: &str) -> PlatformResult;
    fn open_containing_folder(&mut self, path: &str) -> PlatformResult;
    fn open_url(&mut self, url: &str) -> PlatformResult;
}

pub struct OpenService<'a, A>
where
    A: OpenAdapter,
{
    adapter: &'a mut A,
}

impl<'a, A> OpenService<'a, A>
where
    A: OpenAdapter,
{
    pub fn new(adapter: &'a mut A) -> Self {
        Self { adapter }
    }

    pub fn open(&mut self, target: OpenTarget) -> PlatformResult {
        match target {
            OpenTarget::File(path) => self.adapter.open_file(&path),
            OpenTarget::Directory(path) => self.adapter.open_directory(&path),
            OpenTarget::ContainingFolder(path) => self.adapter.open_containing_folder(&path),
            OpenTarget::Url(url) => self.adapter.open_url(&url),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessCommand {
    pub program: String,
    pub args: Vec<String>,
}

pub struct WindowsTerminalAdapter;

impl WindowsTerminalAdapter {
    pub fn build_command(&self, command: &str) -> PlatformResult<ProcessCommand> {
        Ok(ProcessCommand {
            program: "wt.exe".to_owned(),
            args: vec!["cmd.exe".to_owned(), "/C".to_owned(), command.to_owned()],
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DevelopmentToolAdapter {
    preferred_tools: Vec<String>,
}

impl DevelopmentToolAdapter {
    pub fn new(preferred_tools: Vec<String>) -> Self {
        Self { preferred_tools }
    }

    pub fn build_command(
        &self,
        path: &str,
        available_tools: &[&str],
    ) -> PlatformResult<ProcessCommand> {
        let fallback_order = development_tool_fallbacks();
        let program = self
            .preferred_tools
            .iter()
            .map(String::as_str)
            .chain(fallback_order)
            .find(|candidate| available_tools.contains(candidate))
            .ok_or(PlatformError::NoTerminalAvailable)?;

        Ok(ProcessCommand {
            program: program.to_owned(),
            args: development_tool_args(program, path),
        })
    }
}

fn development_tool_fallbacks() -> [&'static str; 8] {
    [
        "code",
        "cursor",
        "code.cmd",
        "cursor.cmd",
        "code.exe",
        "cursor.exe",
        "open",
        "xdg-open",
    ]
}

fn development_tool_args(program: &str, path: &str) -> Vec<String> {
    match program {
        "open" => vec![
            "-a".to_owned(),
            "Visual Studio Code".to_owned(),
            path.to_owned(),
        ],
        "xdg-open" => vec![path.to_owned()],
        _ => vec!["--reuse-window".to_owned(), path.to_owned()],
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosTerminalAdapter;

impl MacosTerminalAdapter {
    pub fn build_command(&self, command: &str) -> PlatformResult<ProcessCommand> {
        let escaped = command
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', " ");
        Ok(ProcessCommand {
            program: "osascript".to_owned(),
            args: vec![
                "-e".to_owned(),
                format!(
                    "tell application \"Terminal\" to do script \"{}\"\ntell application \"Terminal\" to activate",
                    escaped
                ),
            ],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxTerminalAdapter {
    preferred_terminals: Vec<String>,
}

impl LinuxTerminalAdapter {
    pub fn new(preferred_terminals: Vec<String>) -> Self {
        Self {
            preferred_terminals,
        }
    }

    pub fn build_command(
        &self,
        command: &str,
        available_terminals: &[&str],
    ) -> PlatformResult<ProcessCommand> {
        let fallback_order = [
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
            "xterm",
        ];
        let program = self
            .preferred_terminals
            .iter()
            .map(String::as_str)
            .chain(fallback_order)
            .find(|candidate| available_terminals.contains(candidate))
            .ok_or(PlatformError::NoTerminalAvailable)?;

        Ok(ProcessCommand {
            program: program.to_owned(),
            args: linux_terminal_args(program, command),
        })
    }
}

fn linux_terminal_args(program: &str, command: &str) -> Vec<String> {
    match program {
        "gnome-terminal" => vec![
            "--".to_owned(),
            "sh".to_owned(),
            "-lc".to_owned(),
            command.to_owned(),
        ],
        _ => vec![
            "-e".to_owned(),
            "sh".to_owned(),
            "-lc".to_owned(),
            command.to_owned(),
        ],
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSafetyDecision {
    AllowWithConfirmation,
    RequireStrongConfirmation { reason: String },
    Blocked { reason: String },
}

#[derive(Debug, Clone, Default)]
pub struct CommandSafetyChecker;

impl CommandSafetyChecker {
    pub fn check(&self, command: &str) -> CommandSafetyDecision {
        let normalized = command.trim().to_lowercase();

        if normalized == "rm -rf /"
            || normalized.starts_with("rm -rf / ")
            || normalized.starts_with("rm -rf /*")
        {
            return CommandSafetyDecision::Blocked {
                reason: "命令会递归删除根目录".to_owned(),
            };
        }

        if normalized.starts_with("sudo ")
            || normalized.contains(" mkfs")
            || normalized.starts_with("mkfs")
            || normalized.contains(" diskpart")
            || normalized.starts_with("diskpart")
        {
            return CommandSafetyDecision::RequireStrongConfirmation {
                reason: "命令会提升权限或修改系统状态".to_owned(),
            };
        }

        CommandSafetyDecision::AllowWithConfirmation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPress {
    KeyDown(HotkeyKey),
    KeyUp(HotkeyKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyKey {
    Shift,
    Control,
    Alt,
    Command,
    Character(char),
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Function(u8),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyModifier {
    Shift,
    Control,
    Alt,
    Command,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WakeShortcut {
    #[default]
    DoubleShift,
    Chord {
        modifiers: Vec<HotkeyModifier>,
        key: HotkeyKey,
    },
}

impl WakeShortcut {
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim();
        if normalized.eq_ignore_ascii_case("Shift+Shift") {
            return Some(Self::DoubleShift);
        }

        let mut modifiers = Vec::new();
        let mut key = None;
        for part in normalized
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            if let Some(modifier) = parse_hotkey_modifier(part) {
                if !modifiers.contains(&modifier) {
                    modifiers.push(modifier);
                }
            } else if key.is_none() {
                key = parse_hotkey_key(part);
            } else {
                return None;
            }
        }

        let key = key?;
        if modifiers.is_empty() || is_modifier_key(key) {
            return None;
        }
        Some(Self::Chord { modifiers, key })
    }

    pub fn display_label(&self) -> String {
        match self {
            Self::DoubleShift => "Shift+Shift".to_owned(),
            Self::Chord { modifiers, key } => modifiers
                .iter()
                .map(|modifier| match modifier {
                    HotkeyModifier::Shift => "Shift".to_owned(),
                    HotkeyModifier::Control => "Control".to_owned(),
                    HotkeyModifier::Alt => "Alt".to_owned(),
                    HotkeyModifier::Command => "Command".to_owned(),
                })
                .chain(std::iter::once(display_hotkey_key(*key)))
                .collect::<Vec<_>>()
                .join("+"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyState {
    shortcut: WakeShortcut,
    last_shift_at_ms: Option<u64>,
    double_shift_window_ms: u64,
    chord_window_ms: u64,
    pressed_modifiers: Vec<(HotkeyModifier, u64)>,
}

impl Default for HotkeyState {
    fn default() -> Self {
        Self {
            shortcut: WakeShortcut::default(),
            last_shift_at_ms: None,
            double_shift_window_ms: 500,
            chord_window_ms: 500,
            pressed_modifiers: Vec::new(),
        }
    }
}

impl HotkeyState {
    pub fn with_shortcut(shortcut: WakeShortcut) -> Self {
        Self {
            shortcut,
            ..Self::default()
        }
    }

    pub fn set_shortcut(&mut self, shortcut: WakeShortcut) {
        if self.shortcut != shortcut {
            self.shortcut = shortcut;
            self.last_shift_at_ms = None;
            self.pressed_modifiers.clear();
        }
    }

    pub fn register_key_press(&mut self, key: KeyPress, timestamp_ms: u64) -> bool {
        match self.shortcut.clone() {
            WakeShortcut::DoubleShift => self.register_double_shift(key, timestamp_ms),
            WakeShortcut::Chord {
                modifiers,
                key: wake_key,
            } => self.register_chord(key, timestamp_ms, &modifiers, wake_key),
        }
    }

    fn register_double_shift(&mut self, key: KeyPress, timestamp_ms: u64) -> bool {
        match key {
            KeyPress::KeyDown(HotkeyKey::Shift) => {
                let should_show = self.last_shift_at_ms.is_some_and(|last| {
                    timestamp_ms.saturating_sub(last) <= self.double_shift_window_ms
                });
                self.last_shift_at_ms = Some(timestamp_ms);
                should_show
            }
            KeyPress::KeyDown(_) => {
                self.last_shift_at_ms = None;
                false
            }
            KeyPress::KeyUp(_) => false,
        }
    }

    fn register_chord(
        &mut self,
        key: KeyPress,
        timestamp_ms: u64,
        modifiers: &[HotkeyModifier],
        wake_key: HotkeyKey,
    ) -> bool {
        match key {
            KeyPress::KeyDown(pressed_key) => {
                if let Some(modifier) = modifier_for_key(pressed_key) {
                    if let Some((_, pressed_at)) = self
                        .pressed_modifiers
                        .iter_mut()
                        .find(|(current, _)| *current == modifier)
                    {
                        *pressed_at = timestamp_ms;
                    } else {
                        self.pressed_modifiers.push((modifier, timestamp_ms));
                    }
                    false
                } else if pressed_key == wake_key {
                    let should_show = modifiers.iter().all(|modifier| {
                        self.pressed_modifiers
                            .iter()
                            .find(|(current, _)| current == modifier)
                            .is_some_and(|(_, pressed_at)| {
                                timestamp_ms.saturating_sub(*pressed_at) <= self.chord_window_ms
                            })
                    });
                    self.pressed_modifiers.clear();
                    should_show
                } else {
                    self.pressed_modifiers.clear();
                    false
                }
            }
            KeyPress::KeyUp(released_key) => {
                if let Some(modifier) = modifier_for_key(released_key) {
                    self.pressed_modifiers
                        .retain(|(current, _)| *current != modifier);
                }
                false
            }
        }
    }
}

fn parse_hotkey_modifier(part: &str) -> Option<HotkeyModifier> {
    match part.to_ascii_lowercase().as_str() {
        "shift" => Some(HotkeyModifier::Shift),
        "control" | "ctrl" => Some(HotkeyModifier::Control),
        "alt" | "option" => Some(HotkeyModifier::Alt),
        "command" | "cmd" | "meta" | "super" => Some(HotkeyModifier::Command),
        _ => None,
    }
}

fn parse_hotkey_key(part: &str) -> Option<HotkeyKey> {
    let lower = part.to_ascii_lowercase();
    match lower.as_str() {
        "space" => Some(HotkeyKey::Space),
        "enter" | "return" => Some(HotkeyKey::Enter),
        "escape" | "esc" => Some(HotkeyKey::Escape),
        "tab" => Some(HotkeyKey::Tab),
        "backspace" => Some(HotkeyKey::Backspace),
        "delete" | "del" => Some(HotkeyKey::Delete),
        "arrowup" | "up" => Some(HotkeyKey::ArrowUp),
        "arrowdown" | "down" => Some(HotkeyKey::ArrowDown),
        "arrowleft" | "left" => Some(HotkeyKey::ArrowLeft),
        "arrowright" | "right" => Some(HotkeyKey::ArrowRight),
        _ if lower.len() == 1 => lower
            .chars()
            .next()
            .map(|ch| HotkeyKey::Character(ch.to_ascii_uppercase())),
        _ if lower.starts_with('f') => lower[1..]
            .parse::<u8>()
            .ok()
            .filter(|number| (1..=24).contains(number))
            .map(HotkeyKey::Function),
        _ => None,
    }
}

fn display_hotkey_key(key: HotkeyKey) -> String {
    match key {
        HotkeyKey::Character(ch) => ch.to_string(),
        HotkeyKey::Space => "Space".to_owned(),
        HotkeyKey::Enter => "Enter".to_owned(),
        HotkeyKey::Escape => "Escape".to_owned(),
        HotkeyKey::Tab => "Tab".to_owned(),
        HotkeyKey::Backspace => "Backspace".to_owned(),
        HotkeyKey::Delete => "Delete".to_owned(),
        HotkeyKey::ArrowUp => "ArrowUp".to_owned(),
        HotkeyKey::ArrowDown => "ArrowDown".to_owned(),
        HotkeyKey::ArrowLeft => "ArrowLeft".to_owned(),
        HotkeyKey::ArrowRight => "ArrowRight".to_owned(),
        HotkeyKey::Function(number) => format!("F{number}"),
        HotkeyKey::Shift => "Shift".to_owned(),
        HotkeyKey::Control => "Control".to_owned(),
        HotkeyKey::Alt => "Alt".to_owned(),
        HotkeyKey::Command => "Command".to_owned(),
        HotkeyKey::Other => "Other".to_owned(),
    }
}

fn is_modifier_key(key: HotkeyKey) -> bool {
    matches!(
        key,
        HotkeyKey::Shift | HotkeyKey::Control | HotkeyKey::Alt | HotkeyKey::Command
    )
}

fn modifier_for_key(key: HotkeyKey) -> Option<HotkeyModifier> {
    match key {
        HotkeyKey::Shift => Some(HotkeyModifier::Shift),
        HotkeyKey::Control => Some(HotkeyModifier::Control),
        HotkeyKey::Alt => Some(HotkeyModifier::Alt),
        HotkeyKey::Command => Some(HotkeyModifier::Command),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherWindowEffect {
    ShowAndFocus,
    Hide,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LauncherWindowState {
    visible: bool,
    focused: bool,
}

impl LauncherWindowState {
    pub fn show(&mut self) {
        self.visible = true;
        self.focused = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.focused = false;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn mark_backgrounded(&mut self) {
        self.visible = true;
        self.focused = false;
    }

    pub fn toggle_for_global_hotkey(&mut self) -> LauncherWindowEffect {
        if self.visible && self.focused {
            self.hide();
            LauncherWindowEffect::Hide
        } else {
            self.show();
            LauncherWindowEffect::ShowAndFocus
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_adapter_uses_available_drives_as_windows_default_index_roots() {
        let adapter = PathAdapter::new(PlatformKind::Windows)
            .with_home_dir("/home/frank")
            .with_user_profile("C:\\Users\\Frank")
            .with_available_drive_roots(vec!["C:\\".to_owned(), "D:\\".to_owned()]);

        assert_eq!(
            adapter.default_index_roots(),
            vec!["C:\\".to_owned(), "D:\\".to_owned()]
        );
    }

    #[test]
    fn path_adapter_falls_back_to_user_profile_without_windows_drives() {
        let adapter = PathAdapter::new(PlatformKind::Windows)
            .with_home_dir("/home/frank")
            .with_user_profile("C:\\Users\\Frank");

        assert_eq!(
            adapter.default_index_roots(),
            vec!["C:\\Users\\Frank".to_owned()]
        );
    }

    #[test]
    fn path_adapter_uses_home_dir_as_linux_default_index_root() {
        let adapter = PathAdapter::new(PlatformKind::Linux).with_home_dir("/home/frank");

        assert_eq!(
            adapter.default_index_roots(),
            vec!["/home/frank".to_owned()]
        );
    }

    #[test]
    fn path_adapter_uses_home_dir_as_macos_development_default_index_root() {
        let adapter = PathAdapter::new(PlatformKind::Macos).with_home_dir("/Users/frank");

        assert_eq!(
            adapter.default_index_roots(),
            vec!["/Users/frank".to_owned()]
        );
    }

    #[derive(Default)]
    struct RecordingOpenAdapter {
        opened_files: Vec<String>,
        opened_directories: Vec<String>,
        opened_containing_folders: Vec<String>,
        opened_urls: Vec<String>,
    }

    impl OpenAdapter for RecordingOpenAdapter {
        fn open_file(&mut self, path: &str) -> PlatformResult {
            self.opened_files.push(path.to_owned());
            Ok(())
        }

        fn open_directory(&mut self, path: &str) -> PlatformResult {
            self.opened_directories.push(path.to_owned());
            Ok(())
        }

        fn open_containing_folder(&mut self, path: &str) -> PlatformResult {
            self.opened_containing_folders.push(path.to_owned());
            Ok(())
        }

        fn open_url(&mut self, url: &str) -> PlatformResult {
            self.opened_urls.push(url.to_owned());
            Ok(())
        }
    }

    #[test]
    fn open_service_routes_file_directory_containing_folder_and_url_to_adapter() {
        let mut adapter = RecordingOpenAdapter::default();
        let mut service = OpenService::new(&mut adapter);

        service
            .open(OpenTarget::File("/tmp/readme.md".to_owned()))
            .unwrap();
        service
            .open(OpenTarget::Directory("/tmp/docs".to_owned()))
            .unwrap();
        service
            .open(OpenTarget::ContainingFolder("/tmp/readme.md".to_owned()))
            .unwrap();
        service
            .open(OpenTarget::Url("https://example.com".to_owned()))
            .unwrap();

        assert_eq!(adapter.opened_files, ["/tmp/readme.md"]);
        assert_eq!(adapter.opened_directories, ["/tmp/docs"]);
        assert_eq!(adapter.opened_containing_folders, ["/tmp/readme.md"]);
        assert_eq!(adapter.opened_urls, ["https://example.com"]);
    }

    #[test]
    fn windows_terminal_adapter_builds_wt_command() {
        let command = WindowsTerminalAdapter.build_command("git status").unwrap();

        assert_eq!(command.program, "wt.exe");
        assert_eq!(
            command.args,
            vec![
                "cmd.exe".to_owned(),
                "/C".to_owned(),
                "git status".to_owned()
            ]
        );
    }

    #[test]
    fn development_tool_adapter_prefers_configured_tool_when_available() {
        let adapter = DevelopmentToolAdapter::new(vec!["cursor".to_owned()]);
        let command = adapter
            .build_command("/tmp/project", &["code", "cursor"])
            .unwrap();

        assert_eq!(command.program, "cursor");
        assert_eq!(
            command.args,
            vec!["--reuse-window".to_owned(), "/tmp/project".to_owned()]
        );
    }

    #[test]
    fn development_tool_adapter_falls_back_to_code_or_xdg_open() {
        let adapter = DevelopmentToolAdapter::new(Vec::new());
        let code = adapter.build_command("/tmp/project", &["code"]).unwrap();
        let xdg = adapter
            .build_command("/tmp/project", &["xdg-open"])
            .unwrap();

        assert_eq!(code.program, "code");
        assert_eq!(xdg.program, "xdg-open");
        assert_eq!(xdg.args, vec!["/tmp/project".to_owned()]);
    }

    #[test]
    fn linux_terminal_adapter_uses_configured_terminal_when_available() {
        let adapter = LinuxTerminalAdapter::new(vec!["konsole".to_owned()]);
        let command = adapter
            .build_command("git status", &["gnome-terminal", "konsole", "xterm"])
            .unwrap();

        assert_eq!(command.program, "konsole");
        assert_eq!(
            command.args,
            vec![
                "-e".to_owned(),
                "sh".to_owned(),
                "-lc".to_owned(),
                "git status".to_owned()
            ]
        );
    }

    #[test]
    fn macos_terminal_adapter_builds_osascript_command() {
        let command = MacosTerminalAdapter
            .build_command("git status && echo \"ok\"")
            .unwrap();

        assert_eq!(command.program, "osascript");
        assert_eq!(command.args[0], "-e");
        assert!(command.args[1].contains("tell application \"Terminal\""));
        assert!(command.args[1].contains("git status && echo \\\"ok\\\""));
    }

    #[test]
    fn linux_terminal_adapter_falls_back_in_order() {
        let adapter = LinuxTerminalAdapter::new(Vec::new());
        let command = adapter
            .build_command("git status", &["xterm", "gnome-terminal"])
            .unwrap();

        assert_eq!(command.program, "gnome-terminal");
        assert_eq!(
            command.args,
            vec![
                "--".to_owned(),
                "sh".to_owned(),
                "-lc".to_owned(),
                "git status".to_owned()
            ]
        );
    }

    #[test]
    fn command_safety_allows_ordinary_commands_with_confirmation() {
        let checker = CommandSafetyChecker;

        assert_eq!(
            checker.check("git status"),
            CommandSafetyDecision::AllowWithConfirmation
        );
    }

    #[test]
    fn command_safety_blocks_obviously_destructive_commands() {
        let checker = CommandSafetyChecker;

        assert_eq!(
            checker.check("rm -rf /"),
            CommandSafetyDecision::Blocked {
                reason: "命令会递归删除根目录".to_owned()
            }
        );
    }

    #[test]
    fn command_safety_requires_strong_confirmation_for_risky_commands() {
        let checker = CommandSafetyChecker;

        assert_eq!(
            checker.check("sudo apt update"),
            CommandSafetyDecision::RequireStrongConfirmation {
                reason: "命令会提升权限或修改系统状态".to_owned()
            }
        );
    }

    #[test]
    fn hotkey_state_detects_double_shift_within_window() {
        let mut state = HotkeyState::default();

        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Shift), 1_000));
        assert!(state.register_key_press(KeyPress::KeyDown(HotkeyKey::Shift), 1_300));
    }

    #[test]
    fn hotkey_state_ignores_slow_or_interrupted_shift_presses() {
        let mut state = HotkeyState::default();

        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Shift), 1_000));
        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Shift), 2_000));
        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Other), 2_100));
        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Shift), 2_200));
    }

    #[test]
    fn wake_shortcut_parses_double_shift_and_chords() {
        assert_eq!(
            WakeShortcut::parse("Shift+Shift"),
            Some(WakeShortcut::DoubleShift)
        );
        assert_eq!(
            WakeShortcut::parse("Control+Space"),
            Some(WakeShortcut::Chord {
                modifiers: vec![HotkeyModifier::Control],
                key: HotkeyKey::Space,
            })
        );
        assert_eq!(
            WakeShortcut::parse("Command+Shift+K").map(|shortcut| shortcut.display_label()),
            Some("Command+Shift+K".to_owned())
        );
    }

    #[test]
    fn wake_shortcut_rejects_empty_or_bare_modifier_shortcuts() {
        assert_eq!(WakeShortcut::parse(""), None);
        assert_eq!(WakeShortcut::parse("Shift"), None);
        assert_eq!(WakeShortcut::parse("Control+Alt"), None);
        assert_eq!(WakeShortcut::parse("Space"), None);
    }

    #[test]
    fn hotkey_state_detects_configured_chord() {
        let mut state = HotkeyState::with_shortcut(WakeShortcut::Chord {
            modifiers: vec![HotkeyModifier::Control],
            key: HotkeyKey::Space,
        });

        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Control), 1_000));
        assert!(state.register_key_press(KeyPress::KeyDown(HotkeyKey::Space), 1_010));
        assert!(!state.register_key_press(KeyPress::KeyUp(HotkeyKey::Control), 1_020));
        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Space), 1_030));
    }

    #[test]
    fn hotkey_state_ignores_chord_after_modifier_timeout() {
        let mut state = HotkeyState::with_shortcut(WakeShortcut::Chord {
            modifiers: vec![HotkeyModifier::Shift],
            key: HotkeyKey::Space,
        });

        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Shift), 1_000));
        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Space), 1_700));
    }

    #[test]
    fn hotkey_state_clears_chord_after_interrupted_typing() {
        let mut state = HotkeyState::with_shortcut(WakeShortcut::Chord {
            modifiers: vec![HotkeyModifier::Shift],
            key: HotkeyKey::Space,
        });

        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Shift), 1_000));
        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Character('A')), 1_010));
        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Space), 1_020));
    }

    #[test]
    fn hotkey_state_consumes_chord_after_trigger() {
        let mut state = HotkeyState::with_shortcut(WakeShortcut::Chord {
            modifiers: vec![HotkeyModifier::Shift],
            key: HotkeyKey::Space,
        });

        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Shift), 1_000));
        assert!(state.register_key_press(KeyPress::KeyDown(HotkeyKey::Space), 1_010));
        assert!(!state.register_key_press(KeyPress::KeyDown(HotkeyKey::Space), 1_020));
    }

    #[test]
    fn launcher_window_state_tracks_show_and_hide() {
        let mut window = LauncherWindowState::default();

        window.show();
        assert!(window.is_visible());

        window.hide();
        assert!(!window.is_visible());
    }

    #[test]
    fn launcher_window_state_toggles_double_shift_visibility_and_focus() {
        let mut window = LauncherWindowState::default();

        assert_eq!(
            window.toggle_for_global_hotkey(),
            LauncherWindowEffect::ShowAndFocus
        );
        assert!(window.is_visible());
        assert!(window.is_focused());

        assert_eq!(
            window.toggle_for_global_hotkey(),
            LauncherWindowEffect::Hide
        );
        assert!(!window.is_visible());

        window.mark_backgrounded();
        assert_eq!(
            window.toggle_for_global_hotkey(),
            LauncherWindowEffect::ShowAndFocus
        );
        assert!(window.is_visible());
        assert!(window.is_focused());
    }
}
