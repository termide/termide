//! JSON-RPC protocol implementation for LSP communication.
//!
//! LSP uses JSON-RPC 2.0 over stdin/stdout with Content-Length headers.

use std::io::BufRead;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC request ID (can be number or string)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum RequestId {
    Number(u64),
    String(String),
}

impl From<u64> for RequestId {
    fn from(id: u64) -> Self {
        RequestId::Number(id)
    }
}

/// JSON-RPC message (request, response, or notification)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
}

/// JSON-RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new JSON-RPC request
    pub fn new(id: impl Into<RequestId>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC notification (no id, no response expected)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    /// Create a new JSON-RPC notification
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

/// Encode a message with Content-Length header for LSP transport
pub fn encode_message(msg: &impl Serialize) -> Result<String, serde_json::Error> {
    let content = serde_json::to_string(msg)?;
    Ok(format!(
        "Content-Length: {}\r\n\r\n{}",
        content.len(),
        content
    ))
}

/// The largest message body accepted from a server. Real responses stay far
/// below it; a larger announced length is treated as a broken stream rather
/// than allocated, since the allocation alone can abort the process.
pub const MAX_CONTENT_LENGTH: usize = 64 * 1024 * 1024;

/// The longest header line accepted, newline included.
const MAX_HEADER_LINE: u64 = 8 * 1024;

/// The `Content-Length` value of one header line: the name compared
/// case-insensitively, whitespace around the value ignored. `None` for any
/// other header, and for a value that is not a number.
pub fn decode_header(line: &str) -> Option<usize> {
    let (name, value) = line.split_once(':')?;
    if !name.trim().eq_ignore_ascii_case("content-length") {
        return None;
    }
    value.trim().parse().ok()
}

/// Read the next framed message body from `reader`: header lines up to a
/// blank one, then exactly `Content-Length` bytes. Headers other than
/// `Content-Length` are skipped, as are blank lines before the first header.
///
/// `Ok(None)` is the end of the stream between messages. Any error — a
/// missing, unparsable or oversized length, an overlong header line, a
/// stream cut mid-message — means the framing can no longer be trusted and
/// the connection must be dropped.
pub fn read_frame(reader: &mut impl BufRead) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::{Error, ErrorKind, Read};

    let invalid = |message: String| Error::new(ErrorKind::InvalidData, message);
    let mut length: Option<usize> = None;
    let mut seen_header = false;
    let mut line = String::new();
    loop {
        line.clear();
        let read = (&mut *reader).take(MAX_HEADER_LINE).read_line(&mut line)?;
        if read == 0 {
            if seen_header {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof,
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
            continue;
        }
        seen_header = true;
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = Some(
                    decode_header(header)
                        .ok_or_else(|| invalid(format!("bad Content-Length: {}", value.trim())))?,
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
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_message() {
        let notification = JsonRpcNotification::new("test/method", None);
        let encoded = encode_message(&notification).unwrap();
        assert!(encoded.starts_with("Content-Length: "));
        assert!(encoded.contains("\r\n\r\n"));
        assert!(encoded.contains("\"jsonrpc\":\"2.0\""));
    }

    #[test]
    fn test_decode_header() {
        assert_eq!(decode_header("Content-Length: 42\r\n"), Some(42));
        assert_eq!(decode_header("Content-Length: 100"), Some(100));
        assert_eq!(decode_header("Invalid-Header: 42"), None);
        assert_eq!(decode_header("content-length:7"), Some(7));
        assert_eq!(decode_header("CONTENT-LENGTH :  9 "), Some(9));
        assert_eq!(decode_header("Content-Length: lots"), None);
    }

    fn frames(wire: &[u8]) -> Vec<std::io::Result<Option<Vec<u8>>>> {
        let mut reader = std::io::BufReader::new(wire);
        let mut out = Vec::new();
        loop {
            let next = read_frame(&mut reader);
            let done = !matches!(next, Ok(Some(_)));
            out.push(next);
            if done {
                return out;
            }
        }
    }

    #[test]
    fn frames_round_trip_with_extra_headers_and_any_case() {
        let wire = b"\r\ncontent-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}Content-Length:4\r\n\r\nnull";
        let got = frames(wire);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].as_ref().unwrap().as_deref(), Some(&b"{}"[..]));
        assert_eq!(got[1].as_ref().unwrap().as_deref(), Some(&b"null"[..]));
        assert!(matches!(got[2], Ok(None)));
    }

    /// A server announcing an absurd length must not get the client to
    /// allocate it: that aborts the whole app.
    #[test]
    fn an_oversized_length_is_a_protocol_error_not_an_allocation() {
        let wire = b"Content-Length: 99999999999\r\n\r\n{}";
        let err = read_frame(&mut std::io::BufReader::new(&wire[..])).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_header_block_without_length_is_a_protocol_error() {
        let wire = b"Content-Type: x\r\n\r\n{}";
        assert!(read_frame(&mut std::io::BufReader::new(&wire[..])).is_err());
        let wire = b"Content-Length: many\r\n\r\n{}";
        assert!(read_frame(&mut std::io::BufReader::new(&wire[..])).is_err());
    }

    #[test]
    fn an_endless_header_line_is_cut_off() {
        let wire = vec![b'x'; 100_000];
        let err = read_frame(&mut std::io::BufReader::new(wire.as_slice())).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_stream_cut_mid_message_is_an_error_not_a_clean_end() {
        assert!(read_frame(&mut std::io::BufReader::new(&b"Content-Length: 10\r\n"[..])).is_err());
        assert!(read_frame(&mut std::io::BufReader::new(
            &b"Content-Length: 10\r\n\r\n{}"[..]
        ))
        .is_err());
    }

    #[test]
    fn test_request_serialization() {
        let request = JsonRpcRequest::new(1u64, "initialize", None);
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"initialize\""));
    }
}
