use super::Cursor;

/// Action for undo/redo
#[derive(Debug, Clone)]
pub enum Action {
    /// Text insertion
    Insert { position: Cursor, text: String },
    /// Text deletion
    Delete { position: Cursor, text: String },
    /// Action group (for merging consecutive insertions)
    #[allow(dead_code)]
    Group { actions: Vec<Action> },
}

impl Action {
    /// Get inverse action
    pub fn inverse(&self) -> Action {
        match self {
            Action::Insert { position, text } => Action::Delete {
                position: *position,
                text: text.clone(),
            },
            Action::Delete { position, text } => Action::Insert {
                position: *position,
                text: text.clone(),
            },
            Action::Group { actions } => Action::Group {
                actions: actions.iter().rev().map(|a| a.inverse()).collect(),
            },
        }
    }

    /// Check if can merge with another action
    pub fn can_merge_with(&self, other: &Action) -> bool {
        match (self, other) {
            // Merge consecutive character insertions on same line
            (
                Action::Insert {
                    position: pos1,
                    text: text1,
                },
                Action::Insert {
                    position: pos2,
                    text: text2,
                },
            ) => {
                // Check that insertion is on same line and text is single character
                pos1.line == pos2.line
                    && text2.len() == 1
                    && !text2.contains('\n')
                    && !text1.contains('\n')
                    && pos2.column == pos1.column + text1.chars().count()
            }
            // Merge consecutive deletions
            (
                Action::Delete {
                    position: pos1,
                    text: text1,
                },
                Action::Delete {
                    position: pos2,
                    text: text2,
                },
            ) => {
                // Backspace - deletion to the left
                pos1.line == pos2.line
                    && text2.len() == 1
                    && !text2.contains('\n')
                    && !text1.contains('\n')
                    && pos2.column + 1 == pos1.column
            }
            _ => false,
        }
    }

    /// Merge with another action
    pub fn merge(&mut self, other: Action) {
        match (self, other) {
            (
                Action::Insert {
                    position: _,
                    text: text1,
                },
                Action::Insert {
                    position: _,
                    text: text2,
                },
            ) => {
                text1.push_str(&text2);
            }
            (
                Action::Delete {
                    position,
                    text: text1,
                },
                Action::Delete {
                    position: pos2,
                    text: text2,
                },
            ) => {
                // Backspace - add character to beginning
                *position = pos2;
                text1.insert_str(0, &text2);
            }
            _ => {}
        }
    }
}

/// Edit history for undo/redo
#[derive(Debug, Clone)]
pub struct History {
    /// Action stack for undo
    undo_stack: Vec<Action>,
    /// Action stack for redo
    redo_stack: Vec<Action>,
    /// Maximum history size
    max_size: usize,
    /// Current accumulated action
    pending_action: Option<Action>,
}

impl History {
    /// Create a new history
    pub fn new() -> Self {
        Self::with_capacity(1000)
    }

    /// Create history with specified size
    pub fn with_capacity(max_size: usize) -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_size,
            pending_action: None,
        }
    }

    /// Record action to history
    pub fn push(&mut self, action: Action) {
        // Clear redo stack on new action
        self.redo_stack.clear();

        // Try to merge with previous action
        if let Some(pending) = &mut self.pending_action {
            if pending.can_merge_with(&action) {
                pending.merge(action);
                return;
            } else {
                // Cannot merge - save accumulated action
                let completed = self
                    .pending_action
                    .take()
                    .expect("pending_action is Some inside if let Some branch");
                self.undo_stack.push(completed);
            }
        }

        // Start new accumulation
        self.pending_action = Some(action);

        // Limit history size
        if self.undo_stack.len() > self.max_size {
            self.undo_stack.remove(0);
        }
    }

    /// Complete current action group (e.g., on focus loss)
    pub fn commit_pending(&mut self) {
        if let Some(action) = self.pending_action.take() {
            self.undo_stack.push(action);
        }
    }

    /// Undo last action
    pub fn undo(&mut self) -> Option<Action> {
        // First complete current action
        self.commit_pending();

        if let Some(action) = self.undo_stack.pop() {
            let inverse = action.inverse();
            self.redo_stack.push(action);
            Some(inverse)
        } else {
            None
        }
    }

    /// Redo undone action
    pub fn redo(&mut self) -> Option<Action> {
        // Complete current action before redo
        self.commit_pending();

        if let Some(action) = self.redo_stack.pop() {
            // Return original action (not inverse!)
            // Store action in undo_stack for possible subsequent undo
            self.undo_stack.push(action.clone());
            Some(action)
        } else {
            None
        }
    }

    /// Check if undo is possible
    #[allow(dead_code)]
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty() || self.pending_action.is_some()
    }

    /// Check if redo is possible
    #[allow(dead_code)]
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Clear history
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.pending_action = None;
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_undo_redo() {
        let mut history = History::new();

        let action = Action::Insert {
            position: Cursor::at(0, 0),
            text: "hello".to_string(),
        };

        history.push(action);
        history.commit_pending();

        assert!(history.can_undo());
        assert!(!history.can_redo());

        let undo_action = history.undo().unwrap();
        match undo_action {
            Action::Delete { position, text } => {
                assert_eq!(position, Cursor::at(0, 0));
                assert_eq!(text, "hello");
            }
            _ => panic!("Expected Delete action"),
        }

        assert!(!history.can_undo());
        assert!(history.can_redo());

        let redo_action = history.redo().unwrap();
        match redo_action {
            Action::Insert { position, text } => {
                assert_eq!(position, Cursor::at(0, 0));
                assert_eq!(text, "hello");
            }
            _ => panic!("Expected Insert action"),
        }
    }

    #[test]
    fn test_merge_inserts() {
        let mut history = History::new();

        // Insert characters one by one
        history.push(Action::Insert {
            position: Cursor::at(0, 0),
            text: "h".to_string(),
        });
        history.push(Action::Insert {
            position: Cursor::at(0, 1),
            text: "e".to_string(),
        });
        history.push(Action::Insert {
            position: Cursor::at(0, 2),
            text: "l".to_string(),
        });

        history.commit_pending();

        // Should be one action in stack
        assert_eq!(history.undo_stack.len(), 1);

        let undo_action = history.undo().unwrap();
        match undo_action {
            Action::Delete { position, text } => {
                assert_eq!(position, Cursor::at(0, 0));
                assert_eq!(text, "hel");
            }
            _ => panic!("Expected merged Delete action"),
        }
    }

    #[test]
    fn test_merge_deletes() {
        let mut history = History::new();

        // Consecutive backspace
        history.push(Action::Delete {
            position: Cursor::at(0, 3),
            text: "l".to_string(),
        });
        history.push(Action::Delete {
            position: Cursor::at(0, 2),
            text: "l".to_string(),
        });
        history.push(Action::Delete {
            position: Cursor::at(0, 1),
            text: "e".to_string(),
        });

        history.commit_pending();

        let undo_action = history.undo().unwrap();
        match undo_action {
            Action::Insert { position, text } => {
                assert_eq!(position, Cursor::at(0, 1));
                assert_eq!(text, "ell");
            }
            _ => panic!("Expected merged Insert action"),
        }
    }

    #[test]
    fn test_newline_breaks_merge() {
        let mut history = History::new();

        history.push(Action::Insert {
            position: Cursor::at(0, 0),
            text: "h".to_string(),
        });
        history.push(Action::Insert {
            position: Cursor::at(0, 1),
            text: "\n".to_string(),
        });

        history.commit_pending();

        // Newline should break merging
        assert_eq!(history.undo_stack.len(), 2);
    }
}
