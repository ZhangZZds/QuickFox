//! Action dispatching will live here.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    OpenPath {
        path: String,
    },
    OpenContainingFolder {
        path: String,
    },
    CopyText {
        text: String,
    },
    OpenUrl {
        url: String,
    },
    ExecuteCommand {
        command: String,
        requires_confirmation: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionDispatchError {
    CommandRequiresConfirmation,
    Adapter(String),
}

pub type ActionDispatchResult = Result<ActionOutcome, ActionDispatchError>;

pub trait ActionHandler {
    fn open_path(&mut self, path: &str) -> ActionDispatchResult;
    fn copy_text(&mut self, text: &str) -> ActionDispatchResult;
    fn open_url(&mut self, url: &str) -> ActionDispatchResult;
    fn execute_command(&mut self, command: &str) -> ActionDispatchResult;

    fn open_containing_folder(&mut self, path: &str) -> ActionDispatchResult {
        self.open_path(path)
    }
}

pub struct ActionDispatcher<'a, H>
where
    H: ActionHandler,
{
    handler: &'a mut H,
}

impl<'a, H> ActionDispatcher<'a, H>
where
    H: ActionHandler,
{
    pub fn new(handler: &'a mut H) -> Self {
        Self { handler }
    }

    pub fn dispatch(&mut self, action: &Action) -> ActionDispatchResult {
        match action {
            Action::OpenPath { path } => self.handler.open_path(path),
            Action::OpenContainingFolder { path } => self.handler.open_containing_folder(path),
            Action::CopyText { text } => self.handler.copy_text(text),
            Action::OpenUrl { url } => self.handler.open_url(url),
            Action::ExecuteCommand {
                command,
                requires_confirmation,
            } => {
                if !requires_confirmation {
                    return Err(ActionDispatchError::CommandRequiresConfirmation);
                }

                self.handler.execute_command(command)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingActionHandler {
        opened_paths: Vec<String>,
        opened_containing_folders: Vec<String>,
        copied_text: Vec<String>,
        opened_urls: Vec<String>,
        commands: Vec<String>,
    }

    impl ActionHandler for RecordingActionHandler {
        fn open_path(&mut self, path: &str) -> ActionDispatchResult {
            self.opened_paths.push(path.to_owned());
            Ok(ActionOutcome::Completed)
        }

        fn copy_text(&mut self, text: &str) -> ActionDispatchResult {
            self.copied_text.push(text.to_owned());
            Ok(ActionOutcome::Completed)
        }

        fn open_containing_folder(&mut self, path: &str) -> ActionDispatchResult {
            self.opened_containing_folders.push(path.to_owned());
            Ok(ActionOutcome::Completed)
        }

        fn open_url(&mut self, url: &str) -> ActionDispatchResult {
            self.opened_urls.push(url.to_owned());
            Ok(ActionOutcome::Completed)
        }

        fn execute_command(&mut self, command: &str) -> ActionDispatchResult {
            self.commands.push(command.to_owned());
            Ok(ActionOutcome::Completed)
        }
    }

    #[test]
    fn dispatcher_routes_open_copy_url_and_command_actions_to_handler() {
        let mut handler = RecordingActionHandler::default();
        let mut dispatcher = ActionDispatcher::new(&mut handler);

        dispatcher
            .dispatch(&Action::OpenPath {
                path: "/tmp/readme.md".to_owned(),
            })
            .unwrap();
        dispatcher
            .dispatch(&Action::CopyText {
                text: "42".to_owned(),
            })
            .unwrap();
        dispatcher
            .dispatch(&Action::OpenContainingFolder {
                path: "/tmp/readme.md".to_owned(),
            })
            .unwrap();
        dispatcher
            .dispatch(&Action::OpenUrl {
                url: "https://example.com".to_owned(),
            })
            .unwrap();
        dispatcher
            .dispatch(&Action::ExecuteCommand {
                command: "git status".to_owned(),
                requires_confirmation: true,
            })
            .unwrap();

        assert_eq!(handler.opened_paths, ["/tmp/readme.md"]);
        assert_eq!(handler.opened_containing_folders, ["/tmp/readme.md"]);
        assert_eq!(handler.copied_text, ["42"]);
        assert_eq!(handler.opened_urls, ["https://example.com"]);
        assert_eq!(handler.commands, ["git status"]);
    }

    #[test]
    fn dispatcher_refuses_unconfirmed_command_actions() {
        let mut handler = RecordingActionHandler::default();
        let mut dispatcher = ActionDispatcher::new(&mut handler);

        let result = dispatcher.dispatch(&Action::ExecuteCommand {
            command: "git status".to_owned(),
            requires_confirmation: false,
        });

        assert_eq!(
            result,
            Err(ActionDispatchError::CommandRequiresConfirmation)
        );
        assert!(handler.commands.is_empty());
    }
}
