//! Append-only JSONL telemetry support.

use crate::{
    jsonrpc::{RequestId, RpcMethod, ToolCallMeta},
    render::render_path,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Tool completion outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutcome {
    /// The request completed successfully.
    Ok,
    /// The request completed with an error.
    Error,
}

/// Serializable tool error detail.
#[derive(Debug, Clone, Default)]
pub struct ToolErrorDetail {
    /// Error code when one exists.
    pub code: Option<i64>,
    /// Structured error kind.
    pub kind: Option<String>,
    /// Human-facing error message.
    pub message: Option<String>,
}

#[derive(Debug, Default)]
struct PathAggregate {
    request_count: u64,
    error_count: u64,
    total_latency_ms: u128,
    max_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
struct ToolEventRecord {
    event: &'static str,
    ts_unix_ms: u64,
    repo_root: String,
    request_id: Value,
    tool_name: String,
    lsp_method: Option<String>,
    path_hint: Option<String>,
    latency_ms: u64,
    replay_attempts: u8,
    outcome: ToolOutcome,
    error_code: Option<i64>,
    error_kind: Option<String>,
    error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct HotPathsSnapshotRecord {
    event: &'static str,
    ts_unix_ms: u64,
    repo_root: String,
    total_tool_events: u64,
    hottest_paths: Vec<HotPathLine>,
    slowest_paths: Vec<HotPathLine>,
}

#[derive(Debug, Clone, Serialize)]
struct HotPathLine {
    path: String,
    request_count: u64,
    error_count: u64,
    avg_latency_ms: u64,
    max_latency_ms: u64,
}

/// Append-only telemetry log.
#[derive(Debug)]
pub struct TelemetryLog {
    sink: std::fs::File,
    repo_root: String,
    by_path: HashMap<String, PathAggregate>,
    emitted_tool_events: u64,
    snapshot_every: u64,
}

impl TelemetryLog {
    /// Opens or creates a telemetry log file.
    pub fn new(path: &Path, repo_root: &Path, snapshot_every: u64) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let sink = OpenOptions::new().create(true).append(true).open(path)?;
        let repo_root = render_path(repo_root, crate::render::PathStyle::Absolute, None);
        Ok(Self {
            sink,
            repo_root,
            by_path: HashMap::new(),
            emitted_tool_events: 0,
            snapshot_every: snapshot_every.max(1),
        })
    }

    /// Records one tool completion and periodically emits a hot-path snapshot.
    pub fn record_tool_completion(
        &mut self,
        request_id: &RequestId,
        tool_meta: &ToolCallMeta,
        latency_ms: u64,
        replay_attempts: u8,
        outcome: ToolOutcome,
        error: ToolErrorDetail,
    ) -> io::Result<()> {
        let now = unix_ms_now();
        let request_id = request_id.to_json_value();
        let is_error = matches!(outcome, ToolOutcome::Error);
        let ToolErrorDetail {
            code: error_code,
            kind: error_kind,
            message: error_message,
        } = error;
        let record = ToolEventRecord {
            event: "tool_call",
            ts_unix_ms: now,
            repo_root: self.repo_root.clone(),
            request_id,
            tool_name: tool_meta.tool_name.as_str().to_owned(),
            lsp_method: tool_meta
                .lsp_method
                .as_ref()
                .map(RpcMethod::as_str)
                .map(str::to_owned),
            path_hint: tool_meta.path_hint.clone(),
            latency_ms,
            replay_attempts,
            outcome,
            error_code,
            error_kind,
            error_message,
        };
        self.write_json_line(&record)?;

        if let Some(path) = tool_meta.path_hint.as_ref() {
            let aggregate = self.by_path.entry(path.clone()).or_default();
            aggregate.request_count = aggregate.request_count.saturating_add(1);
            aggregate.total_latency_ms = aggregate
                .total_latency_ms
                .saturating_add(u128::from(latency_ms));
            aggregate.max_latency_ms = aggregate.max_latency_ms.max(latency_ms);
            if is_error {
                aggregate.error_count = aggregate.error_count.saturating_add(1);
            }
        }

        self.emitted_tool_events = self.emitted_tool_events.saturating_add(1);
        if self.emitted_tool_events.is_multiple_of(self.snapshot_every) {
            self.write_hot_paths_snapshot()?;
        }
        Ok(())
    }

    /// Emits a hot-path snapshot immediately.
    pub fn write_hot_paths_snapshot(&mut self) -> io::Result<()> {
        let mut hottest = self
            .by_path
            .iter()
            .map(|(path, aggregate)| hot_path_line(path.as_str(), aggregate))
            .collect::<Vec<_>>();
        hottest.sort_by(|left, right| {
            right
                .request_count
                .cmp(&left.request_count)
                .then_with(|| right.max_latency_ms.cmp(&left.max_latency_ms))
                .then_with(|| left.path.cmp(&right.path))
        });
        hottest.truncate(12);

        let mut slowest = self
            .by_path
            .iter()
            .filter(|(_, aggregate)| aggregate.request_count > 0)
            .map(|(path, aggregate)| hot_path_line(path.as_str(), aggregate))
            .collect::<Vec<_>>();
        slowest.sort_by(|left, right| {
            right
                .avg_latency_ms
                .cmp(&left.avg_latency_ms)
                .then_with(|| right.request_count.cmp(&left.request_count))
                .then_with(|| left.path.cmp(&right.path))
        });
        slowest.truncate(12);

        let snapshot = HotPathsSnapshotRecord {
            event: "hot_paths_snapshot",
            ts_unix_ms: unix_ms_now(),
            repo_root: self.repo_root.clone(),
            total_tool_events: self.emitted_tool_events,
            hottest_paths: hottest,
            slowest_paths: slowest,
        };
        self.write_json_line(&snapshot)
    }

    fn write_json_line<T: Serialize>(&mut self, value: &T) -> io::Result<()> {
        let encoded = serde_json::to_vec(value).map_err(|error| {
            io::Error::other(format!("telemetry serialization failed: {error}"))
        })?;
        self.sink.write_all(&encoded)?;
        self.sink.write_all(b"\n")?;
        Ok(())
    }
}

fn hot_path_line(path: &str, aggregate: &PathAggregate) -> HotPathLine {
    let avg_latency_ms = if aggregate.request_count == 0 {
        0
    } else {
        let avg = aggregate.total_latency_ms / u128::from(aggregate.request_count);
        u64::try_from(avg).unwrap_or(u64::MAX)
    };
    HotPathLine {
        path: PathBuf::from(path).display().to_string(),
        request_count: aggregate.request_count,
        error_count: aggregate.error_count,
        avg_latency_ms,
        max_latency_ms: aggregate.max_latency_ms,
    }
}

fn unix_ms_now() -> u64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let millis = since_epoch.as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{TelemetryLog, ToolErrorDetail, ToolOutcome};
    use crate::jsonrpc::{RequestId, ToolCallMeta, ToolName};
    use serde_json::Value;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn writes_tool_events_and_hot_path_snapshots() {
        let dir = tempdir();
        assert!(dir.is_ok());
        let dir = match dir {
            Ok(value) => value,
            Err(_) => return,
        };
        let log_path = dir.path().join("telemetry.jsonl");
        let log = TelemetryLog::new(log_path.as_path(), dir.path(), 1);
        assert!(log.is_ok());
        let mut log = match log {
            Ok(value) => value,
            Err(_) => return,
        };
        let tool_name = ToolName::try_new("hover");
        assert!(tool_name.is_ok());
        let tool_name = match tool_name {
            Ok(value) => value,
            Err(_) => return,
        };
        let record = log.record_tool_completion(
            &RequestId::text("abc"),
            &ToolCallMeta {
                tool_name,
                lsp_method: None,
                path_hint: Some("/tmp/example.rs".to_owned()),
            },
            12,
            0,
            ToolOutcome::Ok,
            ToolErrorDetail::default(),
        );
        assert!(record.is_ok());
        let text = fs::read_to_string(log_path);
        assert!(text.is_ok());
        let text = match text {
            Ok(value) => value,
            Err(_) => return,
        };
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        let first = serde_json::from_str::<Value>(lines[0]);
        assert!(first.is_ok());
        let first = match first {
            Ok(value) => value,
            Err(_) => return,
        };
        assert_eq!(first["event"], "tool_call");
        let second = serde_json::from_str::<Value>(lines[1]);
        assert!(second.is_ok());
        let second = match second {
            Ok(value) => value,
            Err(_) => return,
        };
        assert_eq!(second["event"], "hot_paths_snapshot");
    }
}
