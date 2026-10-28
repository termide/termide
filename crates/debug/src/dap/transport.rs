//! DAP message framing: a `Content-Length` header, a blank line, then the
//! JSON body — the same base protocol as LSP, without JSON-RPC around it.

use std::io::{self, BufRead, Read};

use serde_json::Value;

/// `message` framed for the wire.
pub fn encode(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// The largest message body accepted from an adapter. A larger announced
/// length is treated as a broken stream rather than allocated, since the
/// allocation alone can abort the process.
pub const MAX_CONTENT_LENGTH: usize = 64 * 1024 * 1024;

/// The longest header line accepted, newline included.
const MAX_HEADER_LINE: u64 = 8 * 1024;

/// The next message from `reader`; `None` at the end of the stream between
/// messages. Headers other than `Content-Length` are skipped, its name is
/// matched in any case, and blank lines before a header are skipped. Any
/// error — a missing, unparsable or oversized length, an overlong header
/// line, a stream cut mid-message, a body that is not JSON — means the
/// connection can no longer be trusted.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidData, message);
    let mut length: Option<usize> = None;
    let mut seen_header = false;
    let mut line = String::new();
    loop {
        line.clear();
        let read = (&mut *reader).take(MAX_HEADER_LINE).read_line(&mut line)?;
        if read == 0 {
            if seen_header {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "stream ended inside a message header",
                ));
            }
            return Ok(None);
        }
        if !line.ends_with('\n') && read as u64 == MAX_HEADER_LINE {
            return Err(invalid("header line too long".into()));
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            if seen_header {
                break;
            }
            // Stray blank lines before a header are skipped.
            continue;
        }
        seen_header = true;
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                let value = value.trim();
                length = Some(
                    value
                        .parse()
                        .map_err(|_| invalid(format!("bad Content-Length: {value}")))?,
                );
            }
        }
    }
    let length = length.ok_or_else(|| invalid("message without Content-Length".into()))?;
    if length > MAX_CONTENT_LENGTH {
        return Err(invalid(format!(
            "Content-Length {length} exceeds the {MAX_CONTENT_LENGTH} byte limit"
        )));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
    fn headers_match_in_any_case_and_extra_ones_are_skipped() {
        let wire = b"\r\ncontent-length:2\r\nContent-Type: application/json\r\n\r\n{}";
        let mut reader = io::BufReader::new(&wire[..]);
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({})));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    /// An absurd announced length must not be allocated: that aborts the
    /// whole app. It ends the connection instead.
    #[test]
    fn an_oversized_length_is_a_protocol_error_not_an_allocation() {
        let wire = b"Content-Length: 99999999999\r\n\r\n{}";
        let err = read_message(&mut io::BufReader::new(&wire[..])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_missing_or_bad_length_is_a_protocol_error() {
        for wire in [
            &b"Content-Type: x\r\n\r\n{}"[..],
            &b"Content-Length: many\r\n\r\n{}"[..],
        ] {
            assert!(read_message(&mut io::BufReader::new(wire)).is_err());
        }
    }

    #[test]
    fn an_endless_header_line_is_cut_off() {
        let wire = vec![b'x'; 100_000];
        let err = read_message(&mut io::BufReader::new(wire.as_slice())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_stream_cut_mid_message_is_an_error_not_a_clean_end() {
        for wire in [
            &b"Content-Length: 10\r\n"[..],
            &b"Content-Length: 10\r\n\r\n{}"[..],
        ] {
            assert!(read_message(&mut io::BufReader::new(wire)).is_err());
        }
    }
}
