//! Lightweight JSON-RPC frame helpers.

use crate::normalize::normalize_ascii_token;
use crate::types::InvariantViolation;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, io};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use url::Url;

/// JSON-RPC request identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum RequestId {
    /// Numeric identifier preserved as text for round-trip stability.
    Number(String),
    /// Text identifier.
    Text(String),
}

impl RequestId {
    /// Parses a request ID from JSON.
    #[must_use]
    pub fn from_json_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => Some(Self::Number(number.to_string())),
            Value::String(text) => Some(Self::Text(text.clone())),
            Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
        }
    }

    /// Converts the request ID back to JSON.
    #[must_use]
    pub fn to_json_value(&self) -> Value {
        match self {
            Self::Number(number) => {
                let parsed = serde_json::from_str::<Value>(number);
                match parsed {
                    Ok(value @ Value::Number(_)) => value,
                    Ok(_) | Err(_) => Value::String(number.clone()),
                }
            }
            Self::Text(text) => Value::String(text.clone()),
        }
    }
}

/// JSON-RPC method token.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
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

/// MCP tool name token.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
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
        normalize_ascii_token(self.as_str()) == Self::ADVANCED_LSP_REQUEST_NORMALIZED
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Parsed JSON-RPC frame.
#[derive(Debug, Clone)]
pub struct FramedMessage {
    /// Original payload bytes.
    pub payload: Vec<u8>,
    /// Parsed JSON value.
    pub value: Value,
}

impl FramedMessage {
    /// Parses a JSON-RPC frame payload.
    pub fn parse(payload: Vec<u8>) -> io::Result<Self> {
        let value = serde_json::from_slice::<Value>(&payload).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid JSON-RPC frame payload: {error}"),
            )
        })?;
        if !value.is_object() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSON-RPC frame root must be an object",
            ));
        }
        Ok(Self { payload, value })
    }

    /// Classifies the envelope shape.
    #[must_use]
    pub fn classify(&self) -> RpcEnvelopeKind {
        let method = self
            .value
            .get("method")
            .and_then(RpcMethod::from_json_value);
        let request_id = self.value.get("id").and_then(RequestId::from_json_value);
        match (method, request_id) {
            (Some(method), Some(id)) => RpcEnvelopeKind::Request { id, method },
            (Some(method), None) => RpcEnvelopeKind::Notification { method },
            (None, Some(id)) => RpcEnvelopeKind::Response {
                id,
                has_error: self.value.get("error").is_some(),
            },
            (None, None) => RpcEnvelopeKind::Unknown,
        }
    }
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
    /// Frame shape did not match a recognized envelope.
    Unknown,
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

/// Extracts `tools/call` metadata from a JSON-RPC frame.
#[must_use]
pub fn parse_tool_call_meta(frame: &FramedMessage, rpc_method: &RpcMethod) -> Option<ToolCallMeta> {
    if !rpc_method.is_tools_call() {
        return None;
    }
    let params = frame.value.get("params")?.as_object()?;
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

/// Reads one line-delimited JSON-RPC frame.
pub async fn read_frame<R>(reader: &mut BufReader<R>) -> io::Result<FrameReadOutcome>
where
    R: AsyncRead + Unpin,
{
    loop {
        let mut line = Vec::<u8>::new();
        let bytes_read = reader.read_until(b'\n', &mut line).await?;
        if bytes_read == 0 {
            return Ok(FrameReadOutcome::EndOfStream);
        }

        while line
            .last()
            .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
        {
            let _popped = line.pop();
        }

        if line.is_empty() {
            continue;
        }

        return Ok(FrameReadOutcome::Frame(line));
    }
}

/// Writes one line-delimited JSON-RPC frame.
pub async fn write_frame<W>(writer: &mut W, payload: &[u8]) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
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
        FramedMessage, RequestId, RpcEnvelopeKind, RpcMethod, ToolName, parse_tool_call_meta,
    };
    use serde_json::json;

    #[test]
    fn request_id_round_trips_numeric_and_textual_values() {
        let numeric = RequestId::from_json_value(&json!(42));
        assert!(matches!(numeric, Some(RequestId::Number(ref value)) if value == "42"));

        let textual = RequestId::from_json_value(&json!("abc"));
        assert!(matches!(textual, Some(RequestId::Text(ref value)) if value == "abc"));

        let round_trip = numeric.map(|value| value.to_json_value());
        assert_eq!(round_trip, Some(json!(42)));
    }

    #[test]
    fn method_and_tool_tokens_reject_empty_text() {
        assert!(RpcMethod::try_new("tools/call").is_ok());
        assert!(RpcMethod::try_new("").is_err());
        assert!(ToolName::try_new("hover").is_ok());
        assert!(ToolName::try_new(" ").is_err());
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
}
