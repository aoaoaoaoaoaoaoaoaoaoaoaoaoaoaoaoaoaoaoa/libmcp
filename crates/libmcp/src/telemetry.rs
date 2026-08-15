//! Bounded, record-atomic JSONL telemetry support.

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
    io::{Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

static TELEMETRY_WRITE_LOCK: Mutex<()> = Mutex::new(());

const TELEMETRY_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Durability applied after each complete telemetry record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryFlushPolicy {
    /// Return after appending to the operating-system page cache.
    PageCache,
    /// Flush language-level buffering after each record.
    Flush,
    /// Request data durability from the operating system after each record.
    SyncData,
}

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

#[derive(Debug, Clone, Copy, Default)]
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

/// Bounded JSONL telemetry log.
#[derive(Debug)]
pub struct TelemetryLog {
    sink: std::fs::File,
    repo_root: String,
    by_path: HashMap<String, PathAggregate>,
    emitted_tool_events: u64,
    snapshot_every: u64,
    flush_policy: TelemetryFlushPolicy,
}

impl TelemetryLog {
    /// Opens or creates a telemetry log file.
    pub fn new(
        path: &Path,
        repo_root: &Path,
        snapshot_every: u64,
        flush_policy: TelemetryFlushPolicy,
    ) -> io::Result<Self> {
        if snapshot_every == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "telemetry snapshot interval must be non-zero",
            ));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        let options = options.create(true);
        #[cfg(windows)]
        // Rust deliberately withholds FILE_WRITE_DATA from append handles.
        // The cross-process lock makes an explicit seek-and-write equivalent.
        let options = options.read(true).write(true);
        #[cfg(not(windows))]
        let options = options.append(true);
        let sink = options.open(path)?;
        let repo_root = render_path(repo_root, crate::render::PathStyle::Absolute, None);
        Ok(Self {
            sink,
            repo_root,
            by_path: HashMap::new(),
            emitted_tool_events: 0,
            snapshot_every,
            flush_policy,
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
        let now = unix_ms_now()?;
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
        let next_emitted = self
            .emitted_tool_events
            .checked_add(1)
            .ok_or_else(|| io::Error::other("telemetry event counter exhausted u64"))?;
        let updated_path = if let Some(path) = tool_meta.path_hint.as_ref() {
            let aggregate = self.by_path.get(path).copied().unwrap_or_default();
            let request_count = aggregate
                .request_count
                .checked_add(1)
                .ok_or_else(|| io::Error::other("telemetry path counter exhausted u64"))?;
            let total_latency_ms = aggregate
                .total_latency_ms
                .checked_add(u128::from(latency_ms))
                .ok_or_else(|| io::Error::other("telemetry latency total exhausted u128"))?;
            let error_count = if is_error {
                aggregate
                    .error_count
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("telemetry error counter exhausted u64"))?
            } else {
                aggregate.error_count
            };
            Some((
                path.clone(),
                PathAggregate {
                    request_count,
                    error_count,
                    total_latency_ms,
                    max_latency_ms: aggregate.max_latency_ms.max(latency_ms),
                },
            ))
        } else {
            None
        };

        self.write_json_line(&record)?;
        if let Some((path, aggregate)) = updated_path {
            let _previous = self.by_path.insert(path, aggregate);
        }

        self.emitted_tool_events = next_emitted;
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
            .collect::<io::Result<Vec<_>>>()?;
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
            .collect::<io::Result<Vec<_>>>()?;
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
            ts_unix_ms: unix_ms_now()?,
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
        let mut record = encoded;
        record.push(b'\n');
        let process_lock = TELEMETRY_WRITE_LOCK
            .lock()
            .map_err(|_| io::Error::other("telemetry append lock poisoned"))?;
        lock_file_append(&self.sink)?;
        let write_result =
            bounded_append(&mut self.sink, &record, TELEMETRY_MAX_BYTES).and_then(|()| match self
                .flush_policy
            {
                TelemetryFlushPolicy::PageCache => Ok(()),
                TelemetryFlushPolicy::Flush => self.sink.flush(),
                TelemetryFlushPolicy::SyncData => self.sink.sync_data(),
            });
        let unlock_result = unlock_file_append(&self.sink);
        drop(process_lock);
        write_result.and(unlock_result)
    }
}

fn bounded_append(file: &mut std::fs::File, record: &[u8], max_bytes: u64) -> io::Result<()> {
    let record_bytes = u64::try_from(record.len())
        .map_err(|_| io::Error::other("telemetry record length exceeds u64"))?;
    if record_bytes > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("telemetry record exceeds {max_bytes} byte log bound"),
        ));
    }
    if file.metadata()?.len().saturating_add(record_bytes) > max_bytes {
        file.set_len(0)?;
    }
    let _end = file.seek(SeekFrom::End(0))?;
    file.write_all(record)
}

fn hot_path_line(path: &str, aggregate: &PathAggregate) -> io::Result<HotPathLine> {
    let avg_latency_ms = if aggregate.request_count == 0 {
        0
    } else {
        let avg = aggregate.total_latency_ms / u128::from(aggregate.request_count);
        u64::try_from(avg).map_err(|_| io::Error::other("telemetry average latency exceeds u64"))?
    };
    Ok(HotPathLine {
        path: PathBuf::from(path).display().to_string(),
        request_count: aggregate.request_count,
        error_count: aggregate.error_count,
        avg_latency_ms,
        max_latency_ms: aggregate.max_latency_ms,
    })
}

fn unix_ms_now() -> io::Result<u64> {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| io::Error::other("system clock is before the Unix epoch"))?;
    let millis = since_epoch.as_millis();
    u64::try_from(millis).map_err(|_| io::Error::other("Unix timestamp milliseconds exceed u64"))
}

fn lock_file_append(file: &std::fs::File) -> io::Result<()> {
    file.lock()
}

fn unlock_file_append(file: &std::fs::File) -> io::Result<()> {
    file.unlock()
}

#[cfg(test)]
mod tests {
    use super::{TelemetryFlushPolicy, TelemetryLog, ToolErrorDetail, ToolOutcome, bounded_append};
    use crate::jsonrpc::{RequestId, ToolCallMeta, ToolName};
    use serde_json::Value;
    use std::{fs, thread};
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
        let log = TelemetryLog::new(
            log_path.as_path(),
            dir.path(),
            1,
            TelemetryFlushPolicy::PageCache,
        );
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

    #[test]
    fn concurrent_log_instances_emit_only_intact_records() {
        const WRITERS: u64 = 8;
        const RECORDS_PER_WRITER: u64 = 50;

        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(_) => return,
        };
        let log_path = dir.path().join("concurrent.jsonl");
        let mut writers = Vec::new();
        for writer in 0..WRITERS {
            let log_path = log_path.clone();
            let repo_root = dir.path().to_owned();
            writers.push(thread::spawn(move || {
                let mut log = match TelemetryLog::new(
                    &log_path,
                    &repo_root,
                    u64::MAX,
                    TelemetryFlushPolicy::PageCache,
                ) {
                    Ok(log) => log,
                    Err(_) => return false,
                };
                let tool_name = match ToolName::try_new("concurrent") {
                    Ok(name) => name,
                    Err(_) => return false,
                };
                let meta = ToolCallMeta {
                    tool_name,
                    lsp_method: None,
                    path_hint: None,
                };
                for record in 0..RECORDS_PER_WRITER {
                    let request_id = RequestId::text(format!("{writer}-{record}"));
                    if log
                        .record_tool_completion(
                            &request_id,
                            &meta,
                            record,
                            0,
                            ToolOutcome::Ok,
                            ToolErrorDetail::default(),
                        )
                        .is_err()
                    {
                        return false;
                    }
                }
                true
            }));
        }
        for writer in writers {
            assert!(matches!(writer.join(), Ok(true)));
        }

        let text = match fs::read_to_string(log_path) {
            Ok(text) => text,
            Err(_) => return,
        };
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len() as u64, WRITERS * RECORDS_PER_WRITER);
        assert!(
            lines
                .iter()
                .all(|line| serde_json::from_str::<Value>(line).is_ok())
        );
    }

    #[test]
    fn telemetry_configuration_rejects_zero_interval() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(_) => return,
        };
        assert!(
            TelemetryLog::new(
                &dir.path().join("telemetry.jsonl"),
                dir.path(),
                0,
                TelemetryFlushPolicy::PageCache,
            )
            .is_err()
        );
    }

    #[test]
    fn telemetry_storage_never_crosses_its_byte_bound() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(_) => return,
        };
        let path = dir.path().join("bounded.jsonl");
        let mut options = fs::OpenOptions::new();
        let options = options.create(true);
        #[cfg(windows)]
        let options = options.read(true).write(true);
        #[cfg(not(windows))]
        let options = options.append(true);
        let mut file = match options.open(&path) {
            Ok(file) => file,
            Err(_) => return,
        };
        assert!(bounded_append(&mut file, b"first-record\n", 20).is_ok());
        assert!(bounded_append(&mut file, b"second-record\n", 20).is_ok());
        assert_eq!(
            fs::read(path).ok().as_deref(),
            Some(b"second-record\n".as_slice())
        );
    }
}
