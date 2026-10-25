//! `{name}` placeholders in a custom command's line: a parameter's value or
//! something of the focused panel's (`{file}`, `{line}`, …), put in by
//! Termide before the command runs, quoted for whatever reads the line.
//!
//! Only a name Termide has a value for is replaced; any other text between
//! braces stays as written, so `awk '{print $1}'`, `docker -f '{{.Name}}'`
//! and shell brace expansion are left alone.
//!
//! A value is always taken literally, wherever its placeholder stands: bare,
//! in single quotes or in double quotes, it is quoted to match that place,
//! so a file named `$(rm -rf ~).c` is a name and nothing else.

use std::collections::HashMap;

/// The placeholders Termide fills from the focused panel and the project;
/// a parameter of the same name takes precedence.
pub const CONTEXT_PLACEHOLDERS: &[&str] = &["file", "files", "dir", "line", "col", "project"];

/// A placeholder's value: one word, or several (`{files}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceholderValue {
    One(String),
    Many(Vec<String>),
}

/// The quoting rules of the shell that reads a command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quoting {
    /// sh, bash, zsh and the like.
    Posix,
    /// fish, whose single quotes take `\\` and `\'` as escapes.
    Fish,
    /// Windows PowerShell and PowerShell Core.
    PowerShell,
    /// cmd.exe, which has no literal quoting: `%name%` expands even inside
    /// its double quotes.
    Cmd,
}

/// Where a placeholder stands in the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Within {
    Bare,
    Single,
    Double,
}

/// PowerShell takes the typographic quotes for its own as well.
fn ps_single_quote(c: char) -> bool {
    matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}')
}

fn ps_double_quote(c: char) -> bool {
    matches!(c, '"' | '\u{201C}' | '\u{201D}' | '\u{201E}')
}

/// `value` inside fish's single quotes: `\` and `'` escaped by a `\`.
fn fish_in_single(value: &str) -> String {
    value.replace('\\', r"\\").replace('\'', r"\'")
}

/// `value` inside PowerShell's single quotes: each of them doubled.
fn ps_in_single(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if ps_single_quote(c) {
            out.push(c);
        }
        out.push(c);
    }
    out
}

impl Quoting {
    /// `value` as one literal word standing on its own.
    pub fn quote(self, value: &str) -> String {
        self.quote_within(value, Within::Bare)
    }

    fn quote_within(self, value: &str, within: Within) -> String {
        match (self, within) {
            (Self::Posix, Within::Bare) => format!("'{}'", value.replace('\'', r"'\''")),
            // Out of the quotes, the value in single ones, back in: adjacent
            // quoted parts make one word, and nothing expands in '…', not
            // even an interactive bash's `!`.
            (Self::Posix, Within::Single) => format!("'{}'", Self::Posix.quote(value)),
            (Self::Posix, Within::Double) => format!("\"{}\"", Self::Posix.quote(value)),
            (Self::Fish, Within::Bare) => format!("'{}'", fish_in_single(value)),
            (Self::Fish, Within::Single) => fish_in_single(value),
            (Self::Fish, Within::Double) => format!("\"{}\"", Self::Fish.quote(value)),
            (Self::PowerShell, Within::Bare) => format!("'{}'", ps_in_single(value)),
            (Self::PowerShell, Within::Single) => ps_in_single(value),
            (Self::PowerShell, Within::Double) => {
                let mut out = String::with_capacity(value.len());
                for c in value.chars() {
                    if matches!(c, '`' | '$') || ps_double_quote(c) {
                        out.push('`');
                    }
                    out.push(c);
                }
                out
            }
            // No Windows path holds a `"`; doubled, it is read as one.
            (Self::Cmd, Within::Bare) => format!("\"{}\"", value.replace('"', "\"\"")),
            (Self::Cmd, _) => value.replace('"', "\"\""),
        }
    }

    /// Where each placeholder of `text` stands, in order; `None` for one
    /// escaped so that the shell reads it as written (`\{file}` in sh).
    fn positions(self, text: &str, starts: &[usize]) -> Vec<Option<Within>> {
        let mut within = Within::Bare;
        let mut chars = text.char_indices().peekable();
        let mut out = Vec::with_capacity(starts.len());
        for &start in starts {
            let mut escaped = false;
            while let Some(&(i, c)) = chars.peek() {
                if i >= start {
                    break;
                }
                chars.next();
                let escape = match (self, within) {
                    (Self::Posix, Within::Bare | Within::Double) => c == '\\',
                    (Self::Fish, _) => c == '\\',
                    (Self::PowerShell, Within::Bare | Within::Double) => c == '`',
                    (Self::Cmd, Within::Bare) => c == '^',
                    _ => false,
                };
                if escape {
                    if let Some((j, _)) = chars.next() {
                        escaped = j == start;
                    }
                    continue;
                }
                within = match (self, within) {
                    (Self::Posix | Self::Fish, Within::Bare) if c == '\'' => Within::Single,
                    (Self::Posix | Self::Fish, Within::Bare) if c == '"' => Within::Double,
                    (Self::Posix | Self::Fish, Within::Single) if c == '\'' => Within::Bare,
                    (Self::Posix | Self::Fish, Within::Double) if c == '"' => Within::Bare,
                    (Self::PowerShell, Within::Bare) if ps_single_quote(c) => Within::Single,
                    (Self::PowerShell, Within::Bare) if ps_double_quote(c) => Within::Double,
                    // A doubled quote inside its kind is the quote itself.
                    (Self::PowerShell, Within::Single) if ps_single_quote(c) => {
                        if chars.peek().is_some_and(|&(_, n)| ps_single_quote(n)) {
                            chars.next();
                            Within::Single
                        } else {
                            Within::Bare
                        }
                    }
                    (Self::PowerShell, Within::Double) if ps_double_quote(c) => {
                        if chars.peek().is_some_and(|&(_, n)| ps_double_quote(n)) {
                            chars.next();
                            Within::Double
                        } else {
                            Within::Bare
                        }
                    }
                    (Self::Cmd, Within::Bare) if c == '"' => Within::Double,
                    (Self::Cmd, Within::Double) if c == '"' => Within::Bare,
                    (_, within) => within,
                };
            }
            out.push((!escaped).then_some(within));
        }
        out
    }
}

/// The names written as `{name}` in `text`, in order of first use.
pub fn placeholder_names(text: &str) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for (_, name, _) in placeholders(text) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// `text`, a line `quoting`'s shell reads, with each `{name}` that `values`
/// has replaced by its value quoted to stay literal where it stands; the
/// words of a `Many` value are joined by spaces.
pub fn expand_placeholders(
    text: &str,
    values: &HashMap<String, PlaceholderValue>,
    quoting: Quoting,
) -> String {
    let found: Vec<_> = placeholders(text)
        .filter(|(_, name, _)| values.contains_key(*name))
        .collect();
    let starts: Vec<usize> = found.iter().map(|(start, _, _)| *start).collect();
    let positions = quoting.positions(text, &starts);
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for ((start, name, end), within) in found.into_iter().zip(positions) {
        let Some(within) = within else {
            continue;
        };
        out.push_str(&text[copied..start]);
        let quote = |v: &String| quoting.quote_within(v, within);
        match &values[name] {
            PlaceholderValue::One(v) => out.push_str(&quote(v)),
            PlaceholderValue::Many(vs) => {
                out.push_str(&vs.iter().map(quote).collect::<Vec<_>>().join(" "))
            }
        }
        copied = end;
    }
    out.push_str(&text[copied..]);
    out
}

/// Stands for the `{` of a placeholder escaped in a line while the line is
/// split into words, so that the split, which drops the escape, does not
/// make it a placeholder again.
const ESCAPED_BRACE: char = '\u{E000}';

/// `line`, a program and its arguments written as sh would read them, split
/// into words with the placeholders in them replaced by their values as they
/// are, since no shell reads the words: a word that is a `Many` placeholder
/// alone becomes as many words, and an escaped one (`\{file}`) stays as
/// written, without its escape. `None` when a quote is left open.
pub fn expand_placeholder_line(
    line: &str,
    values: &HashMap<String, PlaceholderValue>,
) -> Option<Vec<String>> {
    let found: Vec<usize> = placeholders(line)
        .filter(|(_, name, _)| values.contains_key(*name))
        .map(|(start, _, _)| start)
        .collect();
    let positions = Quoting::Posix.positions(line, &found);
    let mut marked = line.to_string();
    // From the end, so that the starts before stay where they are.
    for (start, within) in found.into_iter().zip(positions).rev() {
        if within.is_none() {
            marked.replace_range(start..start + 1, &ESCAPED_BRACE.to_string());
        }
    }
    let words = crate::split_words(&marked)?;
    Some(expand_placeholder_words(&words, values))
}

/// Words already split, with the placeholders in them replaced as they
/// are: a word that is a `Many` placeholder alone becomes as many words.
fn expand_placeholder_words(
    words: &[String],
    values: &HashMap<String, PlaceholderValue>,
) -> Vec<String> {
    let mut out = Vec::with_capacity(words.len());
    for word in words {
        let whole = word
            .strip_prefix('{')
            .and_then(|w| w.strip_suffix('}'))
            .and_then(|name| values.get(name));
        if let Some(PlaceholderValue::Many(vs)) = whole {
            out.extend(vs.iter().cloned());
            continue;
        }
        let mut expanded = String::with_capacity(word.len());
        let mut copied = 0;
        let written = |text: &str| text.replace(ESCAPED_BRACE, "{");
        for (start, name, end) in placeholders(word) {
            let Some(value) = values.get(name) else {
                continue;
            };
            expanded.push_str(&written(&word[copied..start]));
            match value {
                PlaceholderValue::One(v) => expanded.push_str(v),
                PlaceholderValue::Many(vs) => expanded.push_str(&vs.join(" ")),
            }
            copied = end;
        }
        expanded.push_str(&written(&word[copied..]));
        out.push(expanded);
    }
    out
}

/// Every `{name}` in `text`: its start, its name and the end of its `}`.
fn placeholders(text: &str) -> impl Iterator<Item = (usize, &str, usize)> {
    text.match_indices('{').filter_map(move |(start, _)| {
        // `${name}` is the shell's own.
        if text[..start].ends_with('$') {
            return None;
        }
        let rest = &text[start + 1..];
        let len = rest.find('}')?;
        let name = &rest[..len];
        is_name(name).then_some((start, name, start + len + 2))
    })
}

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, PlaceholderValue)]) -> HashMap<String, PlaceholderValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn one(v: &str) -> PlaceholderValue {
        PlaceholderValue::One(v.into())
    }

    fn posix(text: &str, v: &HashMap<String, PlaceholderValue>) -> String {
        expand_placeholders(text, v, Quoting::Posix)
    }

    /// What sh makes of `line`: the arguments `printf` gets, one per line.
    #[cfg(unix)]
    fn sh_args(line: &str) -> String {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("printf '[%s]\\n' {line}"))
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn known_names_are_replaced_and_quoted() {
        let v = values(&[("file", one("src/my dir/it's.c")), ("line", one("12"))]);
        assert_eq!(
            posix("p4 edit {file} # {line}", &v),
            r"p4 edit 'src/my dir/it'\''s.c' # '12'"
        );
    }

    #[test]
    fn other_braces_stay_as_written() {
        let v = values(&[("file", one("a"))]);
        for text in [
            "awk '{print $1}'",
            "docker inspect -f '{{.Name}}'",
            "echo {a,b} {} { file } {9x} ${HOME} ${file} {unknown}",
            r"echo \{file}",
        ] {
            assert_eq!(posix(text, &v), text);
        }
        assert_eq!(
            placeholder_names("echo {unknown} {file} {file}"),
            ["unknown", "file"]
        );
    }

    // A value that would run something must stay a name wherever its
    // placeholder stands: bare, in single quotes, in double quotes.
    #[cfg(unix)]
    #[test]
    fn a_value_stays_literal_in_any_quotes() {
        let evil = "a b $(echo X) `echo Y` $HOME ' \" \\ !x";
        let v = values(&[("file", one(evil))]);
        assert_eq!(sh_args(&posix("{file}", &v)), format!("[{evil}]\n"));
        assert_eq!(sh_args(&posix("\"{file}\"", &v)), format!("[{evil}]\n"));
        assert_eq!(sh_args(&posix("'{file}'", &v)), format!("[{evil}]\n"));
        assert_eq!(
            sh_args(&posix("\"at {file} $((1+1))\"", &v)),
            format!("[at {evil} 2]\n")
        );
        assert_eq!(
            sh_args(&posix("'at {file} $((1+1))'", &v)),
            format!("[at {evil} $((1+1))]\n")
        );
        assert_eq!(
            sh_args(&posix(r#"x\"{file}\"y"#, &v)),
            format!("[x\"{evil}\"y]\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn many_values_become_many_words() {
        let v = values(&[(
            "files",
            PlaceholderValue::Many(vec!["a b".into(), "$(c)".into()]),
        )]);
        assert_eq!(posix("p4 add {files}", &v), "p4 add 'a b' '$(c)'");
        assert_eq!(sh_args(&posix("{files}", &v)), "[a b]\n[$(c)]\n");
        // In quotes they stay one word, as everything there does.
        assert_eq!(sh_args(&posix("\"{files}\"", &v)), "[a b $(c)]\n");
        let words: Vec<String> = ["p4", "add", "{files}", "--x={files}"]
            .map(String::from)
            .into();
        assert_eq!(
            expand_placeholder_words(&words, &v),
            ["p4", "add", "a b", "$(c)", "--x=a b $(c)"]
        );
    }

    #[test]
    fn powershell_quotes_for_where_it_stands() {
        let v = values(&[("file", one("a'b $x`\"c"))]);
        let ps = |t| expand_placeholders(t, &v, Quoting::PowerShell);
        assert_eq!(ps("cat {file}"), "cat 'a''b $x`\"c'");
        assert_eq!(ps("cat '{file}'"), "cat 'a''b $x`\"c'");
        assert_eq!(ps("cat \"at {file}\""), "cat \"at a'b `$x```\"c\"");
        // A doubled quote does not end the string it stands in.
        assert_eq!(ps("echo 'it''s {file}'"), "echo 'it''s a''b $x`\"c'");
        assert_eq!(ps("echo `{file}"), "echo `{file}");
    }

    // A `\'` in a value must not end fish's quotes and run the rest.
    #[test]
    fn fish_escapes_inside_its_single_quotes() {
        let v = values(&[("file", one(r"a\'; rm x; '\b"))]);
        let fish = |t| expand_placeholders(t, &v, Quoting::Fish);
        let inner = r"a\\\'; rm x; \'\\b";
        assert_eq!(fish("cat {file}"), format!("cat '{inner}'"));
        assert_eq!(fish("cat 'at {file}'"), format!("cat 'at {inner}'"));
        assert_eq!(
            fish("cat \"at {file}\""),
            format!("cat \"at \"'{inner}'\"\"")
        );
        assert_eq!(fish(r"echo \{file}"), r"echo \{file}");
    }

    #[test]
    fn cmd_quotes_outside_double_quotes_only() {
        let v = values(&[("file", one(r"C:\a b\c.txt"))]);
        let cmd = |t| expand_placeholders(t, &v, Quoting::Cmd);
        assert_eq!(cmd("type {file}"), r#"type "C:\a b\c.txt""#);
        assert_eq!(cmd("type \"{file}\""), r#"type "C:\a b\c.txt""#);
        assert_eq!(cmd("echo ^{file}"), "echo ^{file}");
    }

    #[test]
    fn words_get_values_as_they_are() {
        let v = values(&[("file", one("a 'b'")), ("line", one("3"))]);
        let words: Vec<String> = ["app", "{file}", "--at={line}"].map(String::from).into();
        assert_eq!(
            expand_placeholder_words(&words, &v),
            ["app", "a 'b'", "--at=3"]
        );
    }

    #[test]
    fn a_line_splits_before_values_go_in_and_keeps_escapes() {
        let v = values(&[
            ("file", one("a b")),
            (
                "files",
                PlaceholderValue::Many(vec!["x".into(), "y".into()]),
            ),
        ]);
        assert_eq!(
            expand_placeholder_line(
                r#"app {file} "at {file}" \{file} '{file}' {files} \{files}"#,
                &v
            )
            .unwrap(),
            ["app", "a b", "at a b", "{file}", "a b", "x", "y", "{files}"]
        );
        assert_eq!(expand_placeholder_line("app '{file}", &v), None);
    }
}
