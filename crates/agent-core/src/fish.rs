//! A fish command line read as the POSIX one it means, so the permission
//! checks, written for POSIX, judge what fish would run.
//!
//! Only what decides which commands run, and with which words, is rewritten:
//! a `(…)` or `$(…)` substitution becomes `$(…)`; a single-quoted string,
//! whose `\'` and `\\` are escapes in fish, is requoted; a backquote, plain
//! text in fish, is escaped; `and`/`or` connecting commands are dropped, as
//! the separator before them already splits the line; `not` becomes `!`;
//! and a block's `end` becomes the POSIX word closing it. Anything the
//! rewrite cannot vouch for makes the whole line unreadable: an escape such
//! as `\x2d`, which stands for another character; a `switch` or a
//! `function`; a `<<`, which POSIX would take for a here-document hiding the
//! lines after it; an unbalanced quote, parenthesis or `end`.

/// `command` rewritten as POSIX, or `None` where it cannot be read.
pub(crate) fn as_posix(command: &str) -> Option<String> {
    let chars: Vec<char> = command.chars().collect();
    let mut reader = Reader {
        chars: &chars,
        i: 0,
        out: String::with_capacity(command.len()),
    };
    reader.commands(false)?;
    Some(reader.out)
}

struct Reader<'a> {
    chars: &'a [char],
    i: usize,
    out: String,
}

/// Whether `c` ends a word that could be a reserved word.
fn ends_word(c: Option<char>) -> bool {
    c.is_none_or(|c| c.is_whitespace() || matches!(c, ';' | '|' | '&' | ')' | '<' | '>'))
}

impl Reader<'_> {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.i + ahead).copied()
    }

    /// The reserved word at the reading position, if one is there.
    fn keyword(&self) -> Option<&'static str> {
        const WORDS: [&str; 13] = [
            "and", "or", "not", "begin", "if", "while", "for", "else", "end", "switch", "case",
            "function", "time",
        ];
        WORDS.into_iter().find(|word| {
            let len = word.chars().count();
            self.chars
                .get(self.i..self.i + len)
                .is_some_and(|got| got.iter().copied().eq(word.chars()))
                && ends_word(self.peek(len))
        })
    }

    /// Read commands up to the end, or up to the `)` closing a substitution
    /// when `nested`, which is left unread.
    fn commands(&mut self, nested: bool) -> Option<()> {
        // The POSIX words closing the blocks open here, innermost last.
        let mut blocks: Vec<&'static str> = Vec::new();
        let mut at_command = true;
        while let Some(c) = self.peek(0) {
            if at_command && !c.is_whitespace() {
                if let Some(word) = self.keyword() {
                    self.i += word.len();
                    match word {
                        "and" | "or" => {}
                        "not" => self.out.push('!'),
                        "time" => self.out.push_str("time"),
                        "begin" => {
                            self.out.push('{');
                            blocks.push("}");
                        }
                        "if" | "while" | "for" => {
                            self.out.push_str(word);
                            blocks.push(if word == "if" { "fi" } else { "done" });
                            // A `for` header's words are no command.
                            at_command = word != "for";
                        }
                        // `else if` goes on the block its `if` opened.
                        "else" => {
                            self.out.push_str("else");
                            let rest = &self.chars[self.i..];
                            let blanks = rest.iter().take_while(|c| **c == ' ' || **c == '\t');
                            let after = self.i + blanks.count();
                            if self.chars.get(after..after + 2) == Some(&['i', 'f'])
                                && ends_word(self.chars.get(after + 2).copied())
                            {
                                self.out.extend(&self.chars[self.i..after + 2]);
                                self.i = after + 2;
                            }
                        }
                        "end" => {
                            self.out.push_str(blocks.pop()?);
                            at_command = false;
                        }
                        _ => return None,
                    }
                    continue;
                }
                if c == '!' && ends_word(self.peek(1)) {
                    self.out.push('!');
                    self.i += 1;
                    continue;
                }
                at_command = false;
            }
            match c {
                ')' if nested => break,
                ')' => return None,
                '\'' => self.single_quoted()?,
                '"' => self.double_quoted()?,
                '\\' => self.escape()?,
                '`' => {
                    self.out.push_str("\\`");
                    self.i += 1;
                }
                '(' => self.substitution(1)?,
                '$' if self.peek(1) == Some('(') => self.substitution(2)?,
                '<' if self.peek(1) == Some('<') => return None,
                ';' | '\n' => {
                    self.out.push(c);
                    self.i += 1;
                    at_command = true;
                }
                '|' => {
                    // `|`, `||`, and `&|` before it.
                    self.out.push(c);
                    self.i += 1;
                    at_command = true;
                }
                '&' => {
                    let redirection = self.peek(1) == Some('>')
                        || matches!(
                            self.i.checked_sub(1).map(|p| self.chars[p]),
                            Some('>' | '<')
                        );
                    self.out.push(c);
                    self.i += 1;
                    at_command = !redirection;
                }
                c => {
                    self.out.push(c);
                    self.i += 1;
                }
            }
        }
        Some(())
    }

    /// The substitution whose opening, `(` or `$(`, is `opening` characters
    /// long, as `$(…)`.
    fn substitution(&mut self, opening: usize) -> Option<()> {
        self.i += opening;
        self.out.push_str("$(");
        self.commands(true)?;
        if self.peek(0) != Some(')') {
            return None;
        }
        self.i += 1;
        self.out.push(')');
        Some(())
    }

    /// A single-quoted string, whose `\'` and `\\` are escapes in fish, as
    /// POSIX single quotes.
    fn single_quoted(&mut self) -> Option<()> {
        self.i += 1;
        let mut text = String::new();
        loop {
            match self.peek(0)? {
                '\'' => break,
                '\\' if matches!(self.peek(1), Some('\'' | '\\')) => {
                    text.push(self.peek(1)?);
                    self.i += 2;
                }
                c => {
                    text.push(c);
                    self.i += 1;
                }
            }
        }
        self.i += 1;
        self.out.push('\'');
        self.out.push_str(&text.replace('\'', "'\\''"));
        self.out.push('\'');
        Some(())
    }

    /// A double-quoted string. fish expands `$NAME` and `$(…)` in it, as
    /// POSIX does, but not a backquote, and a backslash escapes only `"`,
    /// `$`, `\` and a line break.
    fn double_quoted(&mut self) -> Option<()> {
        self.out.push('"');
        self.i += 1;
        loop {
            match self.peek(0)? {
                '"' => break,
                '\\' => match self.peek(1)? {
                    c @ ('"' | '$' | '\\' | '\n') => {
                        self.out.push('\\');
                        self.out.push(c);
                        self.i += 2;
                    }
                    // A backslash before anything else is itself.
                    _ => {
                        self.out.push_str("\\\\");
                        self.i += 1;
                    }
                },
                '`' => {
                    self.out.push_str("\\`");
                    self.i += 1;
                }
                '$' if self.peek(1) == Some('(') => self.substitution(2)?,
                c => {
                    self.out.push(c);
                    self.i += 1;
                }
            }
        }
        self.i += 1;
        self.out.push('"');
        Some(())
    }

    /// A backslash outside quotes. Before a letter or a digit it starts an
    /// escape standing for another character (`\n`, `\x2d`, `—`), which
    /// the POSIX reading would take for the letter itself.
    fn escape(&mut self) -> Option<()> {
        let next = self.peek(1)?;
        if next.is_alphanumeric() {
            return None;
        }
        self.out.push('\\');
        self.out.push(next);
        self.i += 2;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_commands_read_as_they_are() {
        for command in [
            "ls -la",
            "cargo build && cargo test",
            "git log | head -n 5",
            "echo \"$HOME\" > /dev/null 2>&1",
            "rg 'fn main' src",
            "make &> log; echo done &",
        ] {
            assert_eq!(as_posix(command).as_deref(), Some(command));
        }
    }

    #[test]
    fn substitutions_become_posix_ones() {
        assert_eq!(as_posix("ls (rm -rf x)").unwrap(), "ls $(rm -rf x)");
        assert_eq!(as_posix("echo a(date)b").unwrap(), "echo a$(date)b");
        assert_eq!(as_posix("echo $(date)").unwrap(), "echo $(date)");
        assert_eq!(
            as_posix("echo (cat (ls); rm x)").unwrap(),
            "echo $(cat $(ls); rm x)"
        );
        assert_eq!(as_posix("echo \"$(date)\"").unwrap(), "echo \"$(date)\"");
        // Neither quotes nor a backquote substitute in fish.
        assert_eq!(
            as_posix("echo '(date)' \"(date)\"").unwrap(),
            "echo '(date)' \"(date)\""
        );
        assert_eq!(as_posix("echo `date`").unwrap(), "echo \\`date\\`");
        assert_eq!(as_posix("echo \"`date`\"").unwrap(), "echo \"\\`date\\`\"");
    }

    #[test]
    fn quotes_are_read_by_fish_rules() {
        // `\'` keeps the string open in fish, so `; rm x` is quoted text.
        assert_eq!(
            as_posix(r"echo 'a\' ; rm x'").unwrap(),
            r"echo 'a'\'' ; rm x'"
        );
        // And where POSIX would see quoted text, fish sees the command.
        assert_eq!(as_posix(r"echo 'a\\' ; rm x").unwrap(), r"echo 'a\' ; rm x");
        assert_eq!(
            as_posix(r#"echo "a\"b" "\(x\)""#).unwrap(),
            r#"echo "a\"b" "\\(x\\)""#
        );
        assert_eq!(as_posix(r"echo \; \(x\)").unwrap(), r"echo \; \(x\)");
    }

    #[test]
    fn connectives_and_blocks_take_their_posix_words() {
        assert_eq!(
            as_posix("make; and make test; or echo no").unwrap(),
            "make;  make test;  echo no"
        );
        assert_eq!(as_posix("not test -f x").unwrap(), "! test -f x");
        assert_eq!(as_posix("begin; ls; end > out").unwrap(), "{; ls; } > out");
        assert_eq!(
            as_posix("for f in *.rs; wc -l $f; end").unwrap(),
            "for f in *.rs; wc -l $f; done"
        );
        assert_eq!(
            as_posix("if test -f a; cat a; else if test -f b; cat b; else; ls; end").unwrap(),
            "if test -f a; cat a; else if test -f b; cat b; else; ls; fi"
        );
        assert_eq!(
            as_posix("while true; ls; end").unwrap(),
            "while true; ls; done"
        );
        // A reserved word in an argument is a word.
        assert_eq!(
            as_posix("echo end and or not").unwrap(),
            "echo end and or not"
        );
        assert_eq!(
            as_posix("ls android endless").unwrap(),
            "ls android endless"
        );
    }

    #[test]
    fn what_cannot_be_vouched_for_is_unreadable() {
        for command in [
            r"find . \x2ddelete",
            r"echo \n",
            "switch $x; case a; rm a; end",
            "function f; rm x; end",
            "cat <<EOF",
            "echo 'open",
            "echo \"open",
            "echo (date",
            "echo date)",
            "ls; end",
            "echo \\",
        ] {
            assert_eq!(as_posix(command), None, "{command}");
        }
    }
}
