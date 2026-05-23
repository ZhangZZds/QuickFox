//! Platform adapters will live here.

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
}
