//! What a confirmed modal hands back, typed.
//!
//! Handlers do not know which modal produced a value, only what it carries:
//! a confirm and a delete prompt both give a `bool`, an input and a commit
//! message both give a `String`. [`ModalOutput::get`] asks for that payload by
//! type, and only types some modal returns are accepted, so asking for one no
//! modal produces does not compile.

use std::path::PathBuf;

use termide_core::ChecklistOutcome;

use crate::dialog::DialogAnswer;
use crate::{
    BookmarkAddResult, CommandConfigResult, CommandParamsResult, ConflictResolution,
    DbFilterResult, DbRowEditResult, InfoActionResult, ProjectAction, SaveAsResult, SettingsResult,
};

/// A confirmed modal's result, one variant per payload type.
#[derive(Debug)]
pub enum ModalOutput {
    /// Info and calendar modals: nothing but the confirmation.
    Unit,
    Bool(bool),
    Index(usize),
    Indices(Vec<usize>),
    Text(String),
    Path(PathBuf),
    Checklist(ChecklistOutcome),
    Dialog(DialogAnswer),
    Conflict(ConflictResolution),
    InfoAction(InfoActionResult),
    Project(ProjectAction),
    SaveAs(SaveAsResult),
    BookmarkAdd(BookmarkAddResult),
    CommandConfig(CommandConfigResult),
    CommandParams(CommandParamsResult),
    DbFilter(DbFilterResult),
    DbRowEdit(DbRowEditResult),
}

impl ModalOutput {
    /// The payload if it is a `T`.
    pub fn get<T: ModalPayload>(&self) -> Option<&T> {
        T::peek(self)
    }
}

/// A type some modal returns as its result.
pub trait ModalPayload: Sized + sealed::Sealed {
    /// Wrap it as a [`ModalOutput`].
    fn wrap(self) -> ModalOutput;
    /// It, if `output` carries one.
    fn peek(output: &ModalOutput) -> Option<&Self>;
}

mod sealed {
    pub trait Sealed {}
}

macro_rules! payload {
    ($($ty:ty => $variant:ident),* $(,)?) => {$(
        impl sealed::Sealed for $ty {}
        impl ModalPayload for $ty {
            fn wrap(self) -> ModalOutput {
                ModalOutput::$variant(self)
            }
            fn peek(output: &ModalOutput) -> Option<&Self> {
                match output {
                    ModalOutput::$variant(value) => Some(value),
                    _ => None,
                }
            }
        }
    )*};
}

payload! {
    bool => Bool,
    usize => Index,
    Vec<usize> => Indices,
    String => Text,
    PathBuf => Path,
    ChecklistOutcome => Checklist,
    DialogAnswer => Dialog,
    ConflictResolution => Conflict,
    InfoActionResult => InfoAction,
    ProjectAction => Project,
    SaveAsResult => SaveAs,
    BookmarkAddResult => BookmarkAdd,
    CommandConfigResult => CommandConfig,
    CommandParamsResult => CommandParams,
    DbFilterResult => DbFilter,
    DbRowEditResult => DbRowEdit,
}

impl sealed::Sealed for () {}
impl ModalPayload for () {
    fn wrap(self) -> ModalOutput {
        ModalOutput::Unit
    }
    fn peek(output: &ModalOutput) -> Option<&Self> {
        matches!(output, ModalOutput::Unit).then_some(&())
    }
}

impl sealed::Sealed for SettingsResult {}
/// The settings modal never confirms with a value: it saves as it goes.
impl ModalPayload for SettingsResult {
    fn wrap(self) -> ModalOutput {
        match self {}
    }
    fn peek(_: &ModalOutput) -> Option<&Self> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_comes_back_only_as_its_own_type() {
        let output = "name".to_string().wrap();
        assert_eq!(output.get::<String>().map(String::as_str), Some("name"));
        assert_eq!(output.get::<bool>(), None);
        assert_eq!(true.wrap().get::<bool>(), Some(&true));
        assert_eq!(().wrap().get::<()>(), Some(&()));
        assert_eq!(().wrap().get::<usize>(), None);
    }
}
