//! Lightweight JSON-RPC frame helpers.

use crate::normalize::fold_ascii_token;
use crate::types::InvariantViolation;
use schemars::JsonSchema;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::{fmt, io};
#[cfg(unix)]
use std::{
    os::fd::AsFd,
    time::{Duration, Instant},
};
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
    /// An object contained the same member name more than once.
    #[error("JSON-RPC frame contains duplicate object member `{0}`")]
    DuplicateObjectMember(String),
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
        let parsed = serde_json::from_slice::<DistinctJsonValue>(&payload)?;
        if let Some(member) = parsed.duplicate_member {
            return Err(FrameParseError::DuplicateObjectMember(member));
        }
        let value = parsed.value;
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

struct DistinctJsonValue {
    value: Value,
    duplicate_member: Option<String>,
}

impl DistinctJsonValue {
    fn unique(value: Value) -> Self {
        Self {
            value,
            duplicate_member: None,
        }
    }
}

impl<'de> Deserialize<'de> for DistinctJsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(DistinctJsonVisitor)
    }
}

struct DistinctJsonVisitor;

impl<'de> Visitor<'de> for DistinctJsonVisitor {
    type Value = DistinctJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(DistinctJsonValue::unique(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(DistinctJsonValue::unique(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(DistinctJsonValue::unique(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(DistinctJsonValue::unique)
            .ok_or_else(|| E::custom("JSON numbers must be finite"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_string(value.to_owned())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(DistinctJsonValue::unique(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(DistinctJsonValue::unique(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(DistinctJsonValue::unique(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        let mut duplicate_member = None;
        while let Some(element) = sequence.next_element::<DistinctJsonValue>()? {
            duplicate_member = duplicate_member.or(element.duplicate_member);
            values.push(element.value);
        }
        Ok(DistinctJsonValue {
            value: Value::Array(values),
            duplicate_member,
        })
    }

    fn visit_map<A>(self, mut members: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut object = Map::new();
        let mut duplicate_member = None;
        while let Some(member) = members.next_key::<String>()? {
            let child = members.next_value::<DistinctJsonValue>()?;
            duplicate_member = duplicate_member.or(child.duplicate_member);
            if object.insert(member.clone(), child.value).is_some() {
                duplicate_member = duplicate_member.or(Some(member));
            }
        }
        Ok(DistinctJsonValue {
            value: Value::Object(object),
            duplicate_member,
        })
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

/// One result of polling a blocking line-delimited JSON-RPC stream.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimedFrameReadOutcome {
    /// A frame payload was read.
    Frame(Vec<u8>),
    /// The stream ended cleanly.
    EndOfStream,
    /// No complete frame arrived before the deadline.
    TimedOut,
}

/// Lossless timed reader for blocking Unix file descriptors.
///
/// The reader retains partial and read-ahead bytes across timeouts. A caller
/// preparing to replace its process image must defer replacement while
/// [`Self::has_buffered_input`] is true; kernel-buffered bytes survive `exec`,
/// but bytes already admitted here do not.
#[cfg(unix)]
pub struct TimedFrameReader<R> {
    reader: R,
    limit: FrameLimit,
    buffer: Vec<u8>,
    consumed: usize,
    eof: bool,
}

#[cfg(unix)]
impl<R> TimedFrameReader<R>
where
    R: io::Read + AsFd,
{
    /// Constructs a timed reader with an explicit per-frame byte limit.
    pub fn new(reader: R, limit: FrameLimit) -> Self {
        Self {
            reader,
            limit,
            buffer: Vec::new(),
            consumed: 0,
            eof: false,
        }
    }

    /// Returns whether bytes have left the kernel stream but not yet formed a
    /// returned frame.
    #[must_use]
    pub fn has_buffered_input(&self) -> bool {
        self.consumed < self.buffer.len()
    }

    /// Waits up to `timeout` for one complete frame or end-of-stream.
    pub fn read_frame(&mut self, timeout: Duration) -> io::Result<TimedFrameReadOutcome> {
        let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "frame poll timeout is too large",
            )
        })?;
        loop {
            if let Some(outcome) = self.extract_frame()? {
                return Ok(outcome);
            }
            if self.eof {
                return Ok(TimedFrameReadOutcome::EndOfStream);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = rustix::event::Timespec::try_from(remaining).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("frame poll timeout is out of range: {error}"),
                )
            })?;
            let ready = {
                let mut descriptors = [rustix::event::PollFd::new(
                    &self.reader,
                    rustix::event::PollFlags::IN,
                )];
                match rustix::event::poll(&mut descriptors, Some(&timeout)) {
                    Ok(ready) => ready,
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(error) => return Err(error.into()),
                }
            };
            if ready == 0 {
                return Ok(TimedFrameReadOutcome::TimedOut);
            }

            self.compact();
            let mut chunk = [0_u8; 8 * 1024];
            let bytes = self.reader.read(&mut chunk)?;
            if bytes == 0 {
                self.eof = true;
            } else {
                self.buffer.extend_from_slice(&chunk[..bytes]);
            }
        }
    }

    fn extract_frame(&mut self) -> io::Result<Option<TimedFrameReadOutcome>> {
        loop {
            let pending = &self.buffer[self.consumed..];
            let Some(delimiter) = pending.iter().position(|byte| *byte == b'\n') else {
                if pending.len() > self.limit.get() {
                    return Err(frame_limit_error(self.limit));
                }
                if self.eof && !pending.is_empty() {
                    let frame = pending.to_vec();
                    self.consumed = self.buffer.len();
                    return Ok(Some(TimedFrameReadOutcome::Frame(frame)));
                }
                return Ok(None);
            };
            if delimiter > self.limit.get() {
                return Err(frame_limit_error(self.limit));
            }
            let start = self.consumed;
            let end = start + delimiter;
            self.consumed = end + 1;
            let mut frame = self.buffer[start..end].to_vec();
            if frame.last() == Some(&b'\r') {
                let _carriage_return = frame.pop();
            }
            if !frame.is_empty() {
                return Ok(Some(TimedFrameReadOutcome::Frame(frame)));
            }
        }
    }

    fn compact(&mut self) {
        if self.consumed == self.buffer.len() {
            self.buffer.clear();
            self.consumed = 0;
        } else if self.consumed >= 8 * 1024 {
            drop(self.buffer.drain(..self.consumed));
            self.consumed = 0;
        }
    }
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
            return Err(frame_limit_error(limit));
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

/// Reads one line-delimited JSON-RPC frame from blocking I/O within an explicit byte limit.
pub fn read_frame_blocking<R>(reader: &mut R, limit: FrameLimit) -> io::Result<FrameReadOutcome>
where
    R: io::BufRead,
{
    let mut line = Vec::<u8>::new();
    loop {
        let buffer = reader.fill_buf()?;
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
            return Err(frame_limit_error(limit));
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
    validate_frame_payload(payload, limit)?;
    writer.write_all(payload).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

/// Writes one line-delimited JSON-RPC frame to blocking I/O within an explicit byte limit.
pub fn write_frame_blocking<W>(writer: &mut W, payload: &[u8], limit: FrameLimit) -> io::Result<()>
where
    W: io::Write,
{
    validate_frame_payload(payload, limit)?;
    writer.write_all(payload)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn validate_frame_payload(payload: &[u8], limit: FrameLimit) -> io::Result<()> {
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
    Ok(())
}

fn frame_limit_error(limit: FrameLimit) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("JSON-RPC frame exceeds {} byte limit", limit.get()),
    )
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
        RpcMethod, ToolName, parse_tool_call_meta, read_frame, read_frame_blocking, write_frame,
        write_frame_blocking,
    };
    #[cfg(unix)]
    use super::{TimedFrameReadOutcome, TimedFrameReader};
    use serde_json::{Number, json};
    use tokio::io::BufReader;
    use url::Url;

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
    fn rejects_duplicate_members_at_every_depth() {
        let root = FramedMessage::parse(
            br#"{"jsonrpc":"2.0","id":1,"id":2,"method":"tools/call"}"#.to_vec(),
        );
        assert!(matches!(
            root,
            Err(FrameParseError::DuplicateObjectMember(member)) if member == "id"
        ));

        let nested = FramedMessage::parse(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"x":1,"x":2}}"#.to_vec(),
        );
        assert!(matches!(
            nested,
            Err(FrameParseError::DuplicateObjectMember(member)) if member == "x"
        ));
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
        let path = std::env::temp_dir().join("libmcp-jsonrpc-example.rs");
        let uri = match Url::from_file_path(&path) {
            Ok(uri) => uri,
            Err(()) => return,
        };
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "advanced_lsp_request",
                "arguments": {
                    "method": "textDocument/hover",
                    "params": {"textDocument": {"uri": uri.as_str()}}
                }
            }
        })
        .to_string()
        .into_bytes();
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
        let expected_path = path.display().to_string();
        assert_eq!(meta.path_hint.as_deref(), Some(expected_path.as_str()));
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

    #[test]
    fn blocking_io_obeys_the_same_frame_bounds() {
        let limit = match FrameLimit::try_new(5) {
            Ok(limit) => limit,
            Err(_) => return,
        };
        let mut reader = std::io::BufReader::new(&b"\n1234\r\n"[..]);
        let outcome = read_frame_blocking(&mut reader, limit);
        assert!(matches!(outcome, Ok(FrameReadOutcome::Frame(payload)) if payload == b"1234"));

        let mut oversized = std::io::BufReader::new(&b"123456\n"[..]);
        let rejected = read_frame_blocking(&mut oversized, limit);
        assert!(matches!(rejected, Err(error) if error.kind() == std::io::ErrorKind::InvalidData));

        let mut sink = Vec::new();
        let written = write_frame_blocking(&mut sink, b"1234", limit);
        assert!(written.is_ok());
        assert_eq!(sink, b"1234\n");
        let rejected = write_frame_blocking(&mut sink, b"123456", limit);
        assert!(matches!(rejected, Err(error) if error.kind() == std::io::ErrorKind::InvalidInput));
    }

    #[cfg(unix)]
    #[test]
    fn timed_reader_retains_partial_and_read_ahead_frames() {
        use std::io::Write as _;
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        let streams = UnixStream::pair();
        assert!(streams.is_ok());
        let (reader, mut writer) = match streams {
            Ok(streams) => streams,
            Err(_) => return,
        };
        let limit = match FrameLimit::try_new(5) {
            Ok(limit) => limit,
            Err(_) => return,
        };
        let mut reader = TimedFrameReader::new(reader, limit);

        let empty = reader.read_frame(Duration::from_millis(1));
        assert!(matches!(empty, Ok(TimedFrameReadOutcome::TimedOut)));
        assert!(!reader.has_buffered_input());

        assert!(writer.write_all(b"12").is_ok());
        let partial = reader.read_frame(Duration::from_millis(1));
        assert!(matches!(partial, Ok(TimedFrameReadOutcome::TimedOut)));
        assert!(reader.has_buffered_input());

        assert!(writer.write_all(b"34\n\n56\n").is_ok());
        let first = reader.read_frame(Duration::from_millis(10));
        assert!(matches!(first, Ok(TimedFrameReadOutcome::Frame(frame)) if frame == b"1234"));
        assert!(reader.has_buffered_input());
        let second = reader.read_frame(Duration::ZERO);
        assert!(matches!(second, Ok(TimedFrameReadOutcome::Frame(frame)) if frame == b"56"));
        assert!(!reader.has_buffered_input());

        drop(writer);
        let end = reader.read_frame(Duration::from_millis(10));
        assert!(matches!(end, Ok(TimedFrameReadOutcome::EndOfStream)));
    }

    #[cfg(unix)]
    #[test]
    fn timed_reader_rejects_oversized_partial_frames() {
        use std::io::Write as _;
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        let streams = UnixStream::pair();
        assert!(streams.is_ok());
        let (reader, mut writer) = match streams {
            Ok(streams) => streams,
            Err(_) => return,
        };
        let limit = match FrameLimit::try_new(5) {
            Ok(limit) => limit,
            Err(_) => return,
        };
        let mut reader = TimedFrameReader::new(reader, limit);
        assert!(writer.write_all(b"123456").is_ok());
        let rejected = reader.read_frame(Duration::from_millis(10));
        assert!(matches!(rejected, Err(error) if error.kind() == std::io::ErrorKind::InvalidData));
    }
}
