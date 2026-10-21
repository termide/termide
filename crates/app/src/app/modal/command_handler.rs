//! Command config modal result handler (create + edit).

use anyhow::Result;
use std::path::{Path, PathBuf};

use termide_config::commands::{CommandMetadata, CommandsMetadata};

use crate::app::App;

impl App {
    /// Handle result from CommandConfigModal (both Create and Edit modes).
    pub(in crate::app) fn handle_command_config_result(
        &mut self,
        result: &termide_modal::CommandConfigResult,
    ) -> Result<()> {
        let config_dir = self.command_config_dir(result.is_project);
        let key = if result.is_edit {
            self.write_command_entry(result, &config_dir, result.name.clone())?;
            result.name.clone()
        } else {
            self.create_command_entry(result, &config_dir)?
        };

        self.state.cache.commands_registry = None;
        self.state.cache.hotkey_table = None;
        // What the user wrote in the form runs without being asked about.
        if result.is_project {
            self.approve_project_command(&key);
        }
        Ok(())
    }

    fn command_config_dir(&self, is_project: bool) -> PathBuf {
        if is_project {
            self.project_root.join(".termide")
        } else {
            termide_config::get_config_dir().unwrap_or_else(|_| PathBuf::from("."))
        }
    }

    /// Create a new command entry in commands.toml. The identifier comes
    /// from the label, so two commands may share it: the new one then gets a
    /// free variant (`build-2`) instead of replacing the other.
    fn create_command_entry(
        &mut self,
        result: &termide_modal::CommandConfigResult,
        config_dir: &Path,
    ) -> Result<String> {
        let key = CommandsMetadata::load(config_dir).free_name(&result.name);
        self.write_command_entry(result, config_dir, key.clone())?;
        Ok(key)
    }

    pub(in crate::app) fn handle_edit_command_config_result(
        &mut self,
        command_name: String,
        was_project: bool,
        result: &termide_modal::CommandConfigResult,
    ) -> Result<()> {
        let source_dir = self.command_config_dir(was_project);
        let target_dir = self.command_config_dir(result.is_project);
        let old_key = command_name;

        let key = if was_project != result.is_project {
            let mut source_metadata = CommandsMetadata::load(&source_dir);
            let mut entry = source_metadata.entries.remove(&old_key).unwrap_or_default();
            apply_result_to_entry(&mut entry, result);
            source_metadata.save(&source_dir)?;

            let mut target_metadata = CommandsMetadata::load(&target_dir);
            let key = target_metadata.free_name(&old_key);
            target_metadata.entries.insert(key.clone(), entry);
            target_metadata.save(&target_dir)?;
            key
        } else {
            self.write_command_entry(result, &target_dir, old_key.clone())?;
            old_key
        };

        // Invalidate caches so the new hotkey/binding takes effect on the next
        // keypress. Without this, `state.cache.hotkey_table` keeps the previous
        // entry — a press of the new chord will be matched against the old
        // ParsedKeyBinding, producing the symptom where the rebind appears to
        // work in-session but reverts after restart (the on-disk value is
        // re-parsed on the next cold start).
        self.state.cache.commands_registry = None;
        self.state.cache.hotkey_table = None;
        if result.is_project {
            self.approve_project_command(&key);
        }
        Ok(())
    }

    fn write_command_entry(
        &self,
        result: &termide_modal::CommandConfigResult,
        config_dir: &Path,
        key: String,
    ) -> Result<()> {
        let mut metadata = CommandsMetadata::load(config_dir);
        let mut entry = metadata.entries.remove(&key).unwrap_or_default();
        apply_result_to_entry(&mut entry, result);
        metadata.entries.insert(key, entry);
        metadata.save(config_dir)?;
        Ok(())
    }
}

/// Apply CommandConfigResult fields to a CommandMetadata entry.
fn apply_result_to_entry(entry: &mut CommandMetadata, result: &termide_modal::CommandConfigResult) {
    // An emptied label is removed, so the menu falls back to the identifier.
    entry.display_name = result.display_name.clone();
    entry.command = result.command.clone();
    entry.mode = Some(result.mode);
    // Only the non-default place is written: unchecked leaves the key out, so
    // a file edited once does not carry `cwd = "panel"` on every command.
    entry.cwd = match result.cwd {
        termide_config::commands::CommandCwd::Project => {
            Some(termide_config::commands::CommandCwd::Project)
        }
        termide_config::commands::CommandCwd::Panel => None,
    };
    entry.key = result.hotkey.clone();
    entry.group = result.group.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_modal::{CommandConfigAction, CommandConfigResult};

    fn result_with_project(is_project: bool) -> CommandConfigResult {
        CommandConfigResult {
            name: "build".to_string(),
            command: Some("cargo test".to_string()),
            display_name: Some("Build".to_string()),
            group: Some("dev".to_string()),
            mode: termide_config::commands::CommandMode::Terminal,
            cwd: termide_config::commands::CommandCwd::Panel,
            hotkey: Some("Ctrl+B".to_string()),
            is_project,
            action: CommandConfigAction::Save,
            is_edit: true,
        }
    }

    #[test]
    fn apply_result_updates_metadata_fields() {
        let mut entry = CommandMetadata::default();
        let result = result_with_project(true);
        apply_result_to_entry(&mut entry, &result);
        assert_eq!(entry.command.as_deref(), Some("cargo test"));
        assert_eq!(entry.display_name.as_deref(), Some("Build"));
        assert_eq!(entry.group.as_deref(), Some("dev"));
        assert_eq!(entry.key.as_deref(), Some("Ctrl+B"));
        assert_eq!(
            entry.mode,
            Some(termide_config::commands::CommandMode::Terminal)
        );
    }

    #[test]
    fn an_emptied_label_is_removed() {
        let mut entry = CommandMetadata {
            display_name: Some("Old".into()),
            ..Default::default()
        };
        let result = CommandConfigResult {
            display_name: None,
            ..result_with_project(false)
        };
        apply_result_to_entry(&mut entry, &result);
        assert_eq!(entry.display_name, None);
    }

    // The box names a place, so only the non-default one is worth a line in
    // commands.toml; unchecking it must take the line away again.
    #[test]
    fn the_project_folder_box_is_the_only_cwd_written() {
        use termide_config::commands::CommandCwd;
        let mut entry = CommandMetadata::default();
        apply_result_to_entry(
            &mut entry,
            &CommandConfigResult {
                cwd: CommandCwd::Project,
                ..result_with_project(true)
            },
        );
        assert_eq!(entry.cwd, Some(CommandCwd::Project));

        apply_result_to_entry(
            &mut entry,
            &CommandConfigResult {
                cwd: CommandCwd::Panel,
                ..result_with_project(true)
            },
        );
        assert_eq!(entry.cwd, None, "the default place stays unwritten");
    }
}
