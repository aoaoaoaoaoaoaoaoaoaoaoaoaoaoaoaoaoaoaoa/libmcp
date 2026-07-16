//! Lightweight JSON-RPC frame helpers.

use crate::normalize::fold_ascii_token;
use crate::types::InvariantViolation;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Number, Value};
use std::{fmt, io};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use url::Url;

/// JSON-RPC request identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RequestId {
    /// Numeric identifier.
    Number(Number),
    /// Text identifier.
    Text(String),
}

impl RequestId {
    /// Constructs a numeric request ID.
    #[must_use]
    pub fn number(number: impl Into<Number>) -> Self {
        Self::Number(number.into())
    }

    /// Constructs a textual request ID.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// Parses a request ID from JSON.
    #[must_use]
    pub fn from_json_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => Some(Self::Number(number.clone())),
            Value::String(text) => Some(Self::Text(text.clone())),
            Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
        }
    }

    /// Converts the request ID back to JSON.
    #[must_use]
    pub fn to_json_value(&self) -> Value {
        match self {
            Self::Number(number) => Value::Number(number.clone()),
            Self::Text(text) => Value::String(text.clone()),
        }
    }
}

/// JSON-RPC method token.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct RpcMethod(String);

impl RpcMethod {
    const INITIALIZE: &'static str = "initialize";
    const INITIALIZED: &'static str = "initialized";
    const NOTIFICATIONS_INITIALIZED: &'static str = "notifications/initialized";
    const TOOLS_CALL: &'static str = "tools/call";

    /// Constructs a method token.
    ///
    /// JSON-RPC method tokens must be non-empty.
    pub fn try_new(method: impl Into<String>) -> Result<Self, InvariantViolation> {
        let method = method.into();
        if method.trim().is_empty() {
            return Err(InvariantViolation::new(
                "JSON-RPC method token must be non-empty",
            ));
        }
        Ok(Self(method))
    }

    /// Returns the MCP `tools/call` method token.
    #[must_use]
    pub fn tools_call() -> Self {
        Self(Self::TOOLS_CALL.to_owned())
    }

    /// Parses a method token from JSON.
    #[must_use]
    pub fn from_json_value(value: &Value) -> Option<Self> {
        value.as_str().and_then(|method| Self::try_new(method).ok())
    }

    /// Returns the method text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Returns whether this is the JSON-RPC initialize request.
    #[must_use]
    pub fn is_initialize(&self) -> bool {
        self.as_str() == Self::INITIALIZE
    }

    /// Returns whether this is an initialized notification spelling.
    #[must_use]
    pub fn is_initialized_notification(&self) -> bool {
        matches!(
            self.as_str(),
            Self::INITIALIZED | Self::NOTIFICATIONS_INITIALIZED
        )
    }

    /// Returns whether this is an MCP tool-call request.
    #[must_use]
    pub fn is_tools_call(&self) -> bool {
        self.as_str() == Self::TOOLS_CALL
    }
}

impl fmt::Display for RpcMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RpcMethod {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let method = String::deserialize(deserializer)?;
        Self::try_new(method).map_err(de::Error::custom)
    }
}

/// MCP tool name token.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct ToolName(String);

impl ToolName {
    const ADVANCED_LSP_REQUEST_NORMALIZED: &'static str = "advancedlsprequest";

    /// Constructs a tool-name token.
    ///
    /// Tool names must be non-empty.
    pub fn try_new(name: impl Into<String>) -> Result<Self, InvariantViolation> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(InvariantViolation::new("tool name must be non-empty"));
        }
        Ok(Self(name))
    }

    /// Returns the tool-name text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Returns whether the tool name denotes the common advanced-LSP proxy.
    #[must_use]
    pub fn is_advanced_lsp_request(&self) -> bool {
        fold_ascii_token(self.as_str()) == Self::ADVANCED_LSP_REQUEST_NORMALIZED
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ToolName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        Self::try_new(name).map_err(de::Error::custom)
    }
}

/// Rejection raised while validating a JSON-RPC frame.
#[derive(Debug, Error)]
pub enum FrameParseError {
    /// The payload was not valid JSON.
    #[error("invalid JSON-RPC frame payload: {0}")]
    InvalidJson(#[from] serde_json::Error),
    /// The frame root was not an object.
    #[error("JSON-RPC frame root must be an object")]
    RootNotObject,
    /// The protocol version was absent or not exactly `2.0`.
    #[error("JSON-RPC frame must declare version 2.0")]
    InvalidVersion,
    /// A request or notification method was not a valid method token.
    #[error("JSON-RPC request method must be a non-empty string")]
    InvalidMethod,
    /// A request or response ID was neither a number nor a string.
    #[error("JSON-RPC request id must be a number or string")]
    InvalidRequestId,
    /// Request parameters were not structured.
    #[error("JSON-RPC params must be an object or array")]
    InvalidParams,
    /// Request and response members formed no unambiguous envelope.
    #[error("JSON-RPC envelope must be exactly one request, notification, or response")]
    AmbiguousEnvelope,
    /// A response error was not a valid JSON-RPC error object.
    #[error("JSON-RPC error must contain an integer code and string message")]
    InvalidError,
}

/// Parsed and validated JSON-RPC frame.
#[derive(Debug, Clone)]
pub struct FramedMessage {
    payload: Vec<u8>,
    value: Value,
    envelope: RpcEnvelopeKind,
}

impl FramedMessage {
    /// Parses and validates a JSON-RPC frame payload.
    pub fn parse(payload: Vec<u8>) -> Result<Self, FrameParseError> {
        let value = serde_json::from_slice::<Value>(&payload)?;
        let object = value.as_object().ok_or(FrameParseError::RootNotObject)?;
        let envelope = validate_envelope(object)?;
        Ok(Self {
            payload,
            value,
            envelope,
        })
    }

    /// Returns the original payload bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Returns the validated JSON value.
    #[must_use]
    pub const fn value(&self) -> &Value {
        &self.value
    }

    /// Returns the validated envelope shape.
    #[must_use]
    pub fn classify(&self) -> RpcEnvelopeKind {
        self.envelope.clone()
    }
}

fn validate_envelope(object: &Map<String, Value>) -> Result<RpcEnvelopeKind, FrameParseError> {
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(FrameParseError::InvalidVersion);
    }

    if let Some(method_value) = object.get("method") {
        if object.contains_key("result") || object.contains_key("error") {
            return Err(FrameParseError::AmbiguousEnvelope);
        }
        if object
            .get("params")
            .is_some_and(|params| !params.is_object() && !params.is_array())
        {
            return Err(FrameParseError::InvalidParams);
        }
        let method =
            RpcMethod::from_json_value(method_value).ok_or(FrameParseError::InvalidMethod)?;
        return match object.get("id") {
            Some(id) => RequestId::from_json_value(id)
                .map(|id| RpcEnvelopeKind::Request { id, method })
                .ok_or(FrameParseError::InvalidRequestId),
            None => Ok(RpcEnvelopeKind::Notification { method }),
        };
    }

    if object.contains_key("params") {
        return Err(FrameParseError::AmbiguousEnvelope);
    }
    let id = object
        .get("id")
        .and_then(RequestId::from_json_value)
        .ok_or(FrameParseError::InvalidRequestId)?;
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");
    if has_result == has_error {
        return Err(FrameParseError::AmbiguousEnvelope);
    }
    if has_error && !object.get("error").is_some_and(valid_error_object) {
        return Err(FrameParseError::InvalidError);
    }
    Ok(RpcEnvelopeKind::Response { id, has_error })
}

fn valid_error_object(error: &Value) -> bool {
    error.as_object().is_some_and(|object| {
        object.get("code").and_then(Value::as_i64).is_some()
            && object.get("message").and_then(Value::as_str).is_some()
    })
}

/// Coarse JSON-RPC envelope classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcEnvelopeKind {
    /// Request with an ID.
    Request {
        /// Request identifier.
        id: RequestId,
        /// Method name.
        method: RpcMethod,
    },
    /// Notification without an ID.
    Notification {
        /// Method name.
        method: RpcMethod,
    },
    /// Response with an ID.
    Response {
        /// Request identifier.
        id: RequestId,
        /// Whether the response carries a JSON-RPC error payload.
        has_error: bool,
    },
}

/// Tool call metadata extracted from a generic `tools/call` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallMeta {
    /// Tool name.
    pub tool_name: ToolName,
    /// Nested LSP method when the tool proxies LSP-style requests.
    pub lsp_method: Option<RpcMethod>,
    /// Best-effort path hint for telemetry grouping.
    pub path_hint: Option<String>,
}

/// One result of reading a line-delimited JSON-RPC stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameReadOutcome {
    /// A frame payload was read.
    Frame(Vec<u8>),
    /// The stream ended cleanly.
    EndOfStream,
}

/// Maximum bytes in one line-delimited frame, excluding the `\n` delimiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameLimit(usize);

impl FrameLimit {
    /// A conservative eight-mebibyte frame limit.
    pub const DEFAULT: Self = Self(8 * 1024 * 1024);

    /// Constructs a non-zero frame limit.
    pub fn try_new(max_bytes: usize) -> Result<Self, InvariantViolation> {
        if max_bytes == 0 {
            return Err(InvariantViolation::new("frame limit must be non-zero"));
        }
        Ok(Self(max_bytes))
    }

    /// Returns the byte limit.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

/// Extracts `tools/call` metadata from a JSON-RPC frame.
#[must_use]
pub fn parse_tool_call_meta(frame: &FramedMessage, rpc_method: &RpcMethod) -> Option<ToolCallMeta> {
    if !rpc_method.is_tools_call() {
        return None;
    }
    let params = frame.value().get("params")?.as_object()?;
    let tool_name = ToolName::try_new(params.get("name")?.as_str()?).ok()?;
    let tool_arguments = params.get("arguments");
    let lsp_method = if tool_name.is_advanced_lsp_request() {
        tool_arguments
            .and_then(Value::as_object)
            .and_then(|arguments| {
                arguments
                    .get("method")
                    .or_else(|| arguments.get("lsp_method"))
                    .or_else(|| arguments.get("lspMethod"))
            })
            .and_then(Value::as_str)
            .and_then(|method| RpcMethod::try_new(method).ok())
    } else {
        None
    };
    let path_hint = tool_arguments.and_then(extract_path_hint_from_value);
    Some(ToolCallMeta {
        tool_name,
        lsp_method,
        path_hint,
    })
}

/// Reads one line-delimited JSON-RPC frame within an explicit byte limit.
pub async fn read_frame<R>(
    reader: &mut BufReader<R>,
    limit: FrameLimit,
) -> io::Result<FrameReadOutcome>
where
    R: AsyncRead + Unpin,
{
    let mut line = Vec::<u8>::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return if line.is_empty() {
                Ok(FrameReadOutcome::EndOfStream)
            } else {
                Ok(FrameReadOutcome::Frame(line))
            };
        }

        let delimiter = buffer.iter().position(|byte| *byte == b'\n');
        let payload_bytes = delimiter.unwrap_or(buffer.len());
        let next_len = line.len().checked_add(payload_bytes);
        if next_len.is_none_or(|length| length > limit.get()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("JSON-RPC frame exceeds {} byte limit", limit.get()),
            ));
        }
        line.extend_from_slice(&buffer[..payload_bytes]);
        let consumed = delimiter.map_or(payload_bytes, |position| position + 1);
        reader.consume(consumed);

        if delimiter.is_none() {
            continue;
        }

        if line.last() == Some(&b'\r') {
            let _carriage_return = line.pop();
        }
        if line.is_empty() {
            continue;
        }
        return Ok(FrameReadOutcome::Frame(line));
    }
}

/// Writes one line-delimited JSON-RPC frame within an explicit byte limit.
pub async fn write_frame<W>(writer: &mut W, payload: &[u8], limit: FrameLimit) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    if payload.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "JSON-RPC frame must not be empty",
        ));
    }
    if payload.len() > limit.get() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("JSON-RPC frame exceeds {} byte limit", limit.get()),
        ));
    }
    if payload.contains(&b'\n') || payload.last() == Some(&b'\r') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "JSON-RPC frame payload must not contain a line delimiter",
        ));
    }
    writer.write_all(payload).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

fn extract_path_hint_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let parsed = parse_nested_json_value(text)?;
            extract_path_hint_from_value(&parsed)
        }
        Value::Object(_) => {
            let direct = extract_direct_path_hint(value);
            if let Some(path) = direct {
                return Some(normalize_path_hint(path.as_str()));
            }
            value
                .as_object()?
                .values()
                .find_map(extract_path_hint_from_value)
        }
        Value::Array(items) => items.iter().find_map(extract_path_hint_from_value),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn parse_nested_json_value(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    let first = trimmed.as_bytes().first()?;
    if !matches!(*first, b'{' | b'[') {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn extract_direct_path_hint(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    for key in ["file_path", "filePath", "path", "uri"] {
        let path = object.get(key).and_then(Value::as_str);
        if let Some(path) = path {
            return Some(path.to_owned());
        }
    }

    let text_document = object.get("textDocument").and_then(Value::as_object);
    if let Some(text_document) = text_document {
        for key in ["uri", "file_path", "filePath", "path"] {
            let path = text_document.get(key).and_then(Value::as_str);
            if let Some(path) = path {
                return Some(path.to_owned());
            }
        }
    }
    None
}

fn normalize_path_hint(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("file://") {
        let parsed = Url::parse(trimmed);
        if let Ok(parsed) = parsed {
            let to_path = parsed.to_file_path();
            if let Ok(path) = to_path {
                return path.display().to_string();
            }
        }
    }
    trimmed.to_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        FrameLimit, FrameParseError, FrameReadOutcome, FramedMessage, RequestId, RpcEnvelopeKind,
        RpcMethod, ToolName, parse_tool_call_meta, read_frame, write_frame,
    };
    use serde_json::{Number, json};
    use tokio::io::BufReader;

    #[test]
    fn request_id_round_trips_numeric_and_textual_values() {
        let numeric = RequestId::from_json_value(&json!(42));
        assert!(
            matches!(numeric, Some(RequestId::Number(ref value)) if value == &Number::from(42))
        );

        let textual = RequestId::from_json_value(&json!("abc"));
        assert!(matches!(textual, Some(RequestId::Text(ref value)) if value == "abc"));

        let round_trip = numeric.map(|value| value.to_json_value());
        assert_eq!(round_trip, Some(json!(42)));
        let serialized = serde_json::to_value(RequestId::number(42));
        assert!(matches!(serialized, Ok(value) if value == json!(42)));
        let deserialized = serde_json::from_value::<RequestId>(json!("abc"));
        assert!(matches!(deserialized, Ok(value) if value == RequestId::text("abc")));
    }

    #[test]
    fn method_and_tool_tokens_reject_empty_text() {
        assert!(RpcMethod::try_new("tools/call").is_ok());
        assert!(RpcMethod::try_new("").is_err());
        assert!(ToolName::try_new("hover").is_ok());
        assert!(ToolName::try_new(" ").is_err());
        assert!(serde_json::from_str::<RpcMethod>(r#""""#).is_err());
        assert!(serde_json::from_str::<ToolName>(r#"" ""#).is_err());
    }

    #[test]
    fn classifies_request_frames() {
        let frame =
            FramedMessage::parse(br#"{"jsonrpc":"2.0","id":7,"method":"tools/call"}"#.to_vec());
        assert!(frame.is_ok());
        let frame = match frame {
            Ok(value) => value,
            Err(_) => return,
        };
        assert!(matches!(
            frame.classify(),
            RpcEnvelopeKind::Request { method, .. } if method.is_tools_call()
        ));
    }

    #[test]
    fn rejects_ambiguous_or_malformed_envelopes() {
        let cases = [
            (br#"{"id":1,"method":"tools/call"}"#.as_slice(), "version"),
            (
                br#"{"jsonrpc":"1.0","id":1,"method":"tools/call"}"#.as_slice(),
                "version",
            ),
            (
                br#"{"jsonrpc":"2.0","id":null,"method":"tools/call"}"#.as_slice(),
                "id",
            ),
            (
                br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":7}"#.as_slice(),
                "params",
            ),
            (
                br#"{"jsonrpc":"2.0","id":1,"result":{},"error":{"code":-1,"message":"x"}}"#
                    .as_slice(),
                "envelope",
            ),
            (
                br#"{"jsonrpc":"2.0","id":1,"error":{"code":"bad","message":"x"}}"#.as_slice(),
                "error",
            ),
        ];

        for (payload, expected) in cases {
            let error = FramedMessage::parse(payload.to_vec());
            assert!(error.is_err(), "{expected} case was accepted");
        }

        let scalar = FramedMessage::parse(b"[]".to_vec());
        assert!(matches!(scalar, Err(FrameParseError::RootNotObject)));
    }

    #[test]
    fn seals_payload_value_and_envelope_together() {
        let payload = br#"{"jsonrpc":"2.0","id":"r1","result":{"ok":true}}"#.to_vec();
        let frame = FramedMessage::parse(payload.clone());
        assert!(frame.is_ok());
        let frame = match frame {
            Ok(frame) => frame,
            Err(_) => return,
        };
        assert_eq!(frame.payload(), payload);
        assert_eq!(frame.value().get("result"), Some(&json!({"ok": true})));
        assert!(matches!(
            frame.classify(),
            RpcEnvelopeKind::Response {
                id,
                has_error: false
            } if id == RequestId::text("r1")
        ));
    }

    #[test]
    fn extracts_tool_call_meta_with_nested_path_hint() {
        let payload = br#"{
            "jsonrpc":"2.0",
            "id":1,
            "method":"tools/call",
            "params":{
                "name":"advanced_lsp_request",
                "arguments":{
                    "method":"textDocument/hover",
                    "params":{"textDocument":{"uri":"file:///tmp/example.rs"}}
                }
            }
        }"#
        .to_vec();
        let frame = FramedMessage::parse(payload);
        assert!(frame.is_ok());
        let frame = match frame {
            Ok(value) => value,
            Err(_) => return,
        };
        let meta = parse_tool_call_meta(&frame, &RpcMethod::tools_call());
        assert!(meta.is_some());
        let meta = match meta {
            Some(value) => value,
            None => return,
        };
        assert_eq!(meta.tool_name.as_str(), "advanced_lsp_request");
        assert_eq!(
            meta.lsp_method.as_ref().map(RpcMethod::as_str),
            Some("textDocument/hover")
        );
        assert_eq!(meta.path_hint.as_deref(), Some("/tmp/example.rs"));
    }

    #[tokio::test]
    async fn reads_frames_without_unbounded_line_growth() {
        let mut reader = BufReader::new(&b"\n1234\r\n"[..]);
        let limit = match FrameLimit::try_new(5) {
            Ok(limit) => limit,
            Err(_) => return,
        };
        let outcome = read_frame(&mut reader, limit).await;
        assert!(matches!(outcome, Ok(FrameReadOutcome::Frame(payload)) if payload == b"1234"));

        let rejected = read_frame(&mut BufReader::new(&b"123456\n"[..]), limit).await;
        assert!(matches!(rejected, Err(error) if error.kind() == std::io::ErrorKind::InvalidData));
    }

    #[tokio::test]
    async fn rejects_unframeable_output_before_writing() {
        let mut sink = tokio::io::sink();
        let limit = match FrameLimit::try_new(16) {
            Ok(limit) => limit,
            Err(_) => return,
        };
        let embedded_newline = write_frame(&mut sink, b"{}\n{}", limit).await;
        assert!(
            matches!(embedded_newline, Err(error) if error.kind() == std::io::ErrorKind::InvalidInput)
        );
        let oversized = write_frame(&mut sink, b"12345678901234567", limit).await;
        assert!(
            matches!(oversized, Err(error) if error.kind() == std::io::ErrorKind::InvalidInput)
        );
    }
}
