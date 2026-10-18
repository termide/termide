//! Shell-like word splitting for command lines and flag lists kept in the
//! config (`commands.toml` commands, debugger flags).

/// Split `text` into words the way a POSIX shell would, for a program and
/// its arguments: single quotes keep everything, double quotes keep all but
/// `\"` and `\\`, a backslash outside quotes escapes the next character.
/// `None` when a quote is left open.
pub fn split_words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            c @ ('"' | '\\' | '$' | '`') => word.push(c),
                            c => {
                                word.push('\\');
                                word.push(c);
                            }
                        },
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(c) = chars.next() {
                    word.push(c);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_like_a_shell() {
        let w = |s| split_words(s).unwrap();
        assert_eq!(
            w("target/debug/app --flag 2"),
            ["target/debug/app", "--flag", "2"]
        );
        assert_eq!(
            w(r#"run 'a b' "c \"d\"" e\ f"#),
            ["run", "a b", "c \"d\"", "e f"]
        );
        assert_eq!(w(r#""a\b""#), [r"a\b"]);
        assert_eq!(w("  ''  "), [""]);
        assert!(split_words("open 'quote").is_none());
    }
}
