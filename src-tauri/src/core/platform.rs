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
}

impl PathAdapter {
    pub fn new(platform: PlatformKind) -> Self {
        Self {
            platform,
            home_dir: None,
            user_profile: None,
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

    pub fn default_index_roots(&self) -> Vec<String> {
        match self.platform {
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
    Shift,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyState {
    last_shift_at_ms: Option<u64>,
    double_shift_window_ms: u64,
}

impl Default for HotkeyState {
    fn default() -> Self {
        Self {
            last_shift_at_ms: None,
            double_shift_window_ms: 500,
        }
    }
}

impl HotkeyState {
    pub fn register_key_press(&mut self, key: KeyPress, timestamp_ms: u64) -> bool {
        match key {
            KeyPress::Other => {
                self.last_shift_at_ms = None;
                false
            }
            KeyPress::Shift => {
                let should_show = self.last_shift_at_ms.is_some_and(|last| {
                    timestamp_ms.saturating_sub(last) <= self.double_shift_window_ms
                });
                self.last_shift_at_ms = Some(timestamp_ms);
                should_show
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LauncherWindowState {
    visible: bool,
}

impl LauncherWindowState {
    pub fn show(&mut self) {
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_adapter_uses_user_profile_as_windows_default_index_root() {
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

        assert!(!state.register_key_press(KeyPress::Shift, 1_000));
        assert!(state.register_key_press(KeyPress::Shift, 1_300));
    }

    #[test]
    fn hotkey_state_ignores_slow_or_interrupted_shift_presses() {
        let mut state = HotkeyState::default();

        assert!(!state.register_key_press(KeyPress::Shift, 1_000));
        assert!(!state.register_key_press(KeyPress::Shift, 2_000));
        assert!(!state.register_key_press(KeyPress::Other, 2_100));
        assert!(!state.register_key_press(KeyPress::Shift, 2_200));
    }

    #[test]
    fn launcher_window_state_tracks_show_and_hide() {
        let mut window = LauncherWindowState::default();

        window.show();
        assert!(window.is_visible());

        window.hide();
        assert!(!window.is_visible());
    }
}
