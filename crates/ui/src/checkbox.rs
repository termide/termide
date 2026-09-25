//! The one checkbox look every control shares: `[✓]` checked, `[ ]` not.

/// The mark inside the brackets: `✓` checked, a blank not.
#[must_use]
pub fn checkbox_mark(checked: bool) -> &'static str {
    if checked {
        "✓"
    } else {
        " "
    }
}

/// The checkbox with its brackets: `[✓]` or `[ ]`.
#[must_use]
pub fn checkbox(checked: bool) -> &'static str {
    if checked {
        "[✓]"
    } else {
        "[ ]"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_states_are_three_columns_wide() {
        for checked in [true, false] {
            assert_eq!(crate::str_display_width(checkbox(checked)), 3);
            assert_eq!(checkbox(checked), format!("[{}]", checkbox_mark(checked)));
        }
    }
}
