//! DAP message framing: a `Content-Length` header, a blank line, then the
//! JSON body — the same base protocol as LSP, without JSON-RPC around it.

use std::io::{self, BufRead};

use serde_json::Value;

/// `message` framed for the wire.
pub fn encode(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// The next message from `reader`; `None` at the end of the stream.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            if length.is_some() {
                break;
            }
            // Stray blank lines before a header are skipped.
            continue;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok();
            }
        }
    }
    let length = length.unwrap_or_default();
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

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
    use serde_json::json;

    #[test]
    fn messages_round_trip_through_the_framing() {
        let a = json!({"seq": 1, "type": "request", "command": "initialize"});
        let b = json!({"seq": 2, "type": "event", "event": "output", "body": {"output": "é\n"}});
        let mut wire = encode(&a);
        wire.extend(encode(&b));
        let mut reader = io::BufReader::new(wire.as_slice());
        assert_eq!(read_message(&mut reader).unwrap(), Some(a));
        assert_eq!(read_message(&mut reader).unwrap(), Some(b));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

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
        assert_eq!(w("  ''  "), [""]);
        assert!(split_words("open 'quote").is_none());
    }
}
