//! Shared test helpers for `libmcp` consumers.

use libmcp::{
    CompletedPendingRequest, DetailLevel, DispatchQueueOutcome, FramedMessage, HostRejection,
    HostSessionKernel, ProbeResolution, ProbeResolutionOutcome, ReplayBudget, ReplayContract,
    ReplayRequeueOutcome, RequestId, ToolProjection,
};
use serde::de::DeserializeOwned;
use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    ops::Deref,
    path::{Path, PathBuf},
};

/// RAII-owned scratch tree for consumer tests.
#[derive(Debug)]
pub struct TestCell(tempfile::TempDir);

impl TestCell {
    /// Creates an isolated scratch tree under the host temporary directory.
    pub fn new(prefix: &str) -> io::Result<Self> {
        tempfile::Builder::new().prefix(prefix).tempdir().map(Self)
    }

    /// Creates an isolated scratch tree beneath an explicit suite root.
    pub fn new_in(prefix: &str, root: &Path) -> io::Result<Self> {
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(root)
            .map(Self)
    }

    /// Returns the live scratch root.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.0.path()
    }

    /// Removes the scratch tree and reports cleanup failure.
    pub fn close(self) -> io::Result<()> {
        self.0.close()
    }

    /// Transfers the scratch tree out of RAII custody.
    #[must_use]
    pub fn persist(self) -> PathBuf {
        self.0.keep()
    }
}

impl AsRef<Path> for TestCell {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Deref for TestCell {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.path()
    }
}

/// Deterministic fake host boundary for recovery conformance tests.
#[derive(Debug, Clone)]
pub struct ChurnHarness {
    kernel: HostSessionKernel,
    dispatched: Vec<FramedMessage>,
}

impl ChurnHarness {
    /// Creates a cold fake host.
    #[must_use]
    pub fn cold() -> Self {
        Self {
            kernel: HostSessionKernel::cold(),
            dispatched: Vec::new(),
        }
    }

    /// Returns the underlying kernel for precise assertions.
    #[must_use]
    pub const fn kernel(&self) -> &HostSessionKernel {
        &self.kernel
    }

    /// Returns every frame that crossed the fake worker dispatch boundary.
    #[must_use]
    pub fn dispatched(&self) -> &[FramedMessage] {
        &self.dispatched
    }

    /// Begins and records one first worker dispatch.
    pub fn dispatch_first(
        &mut self,
        frame: &FramedMessage,
        contract: ReplayContract,
        pending_capacity: usize,
    ) -> Result<RequestId, HostRejection> {
        let id = self
            .kernel
            .begin_request_dispatch(frame, contract, pending_capacity)?;
        self.dispatched.push(frame.clone());
        Ok(id)
    }

    /// Applies worker loss and rebuilds the recovery queue.
    pub fn kill_worker(&mut self, budget: ReplayBudget) -> ReplayRequeueOutcome {
        self.kernel.requeue_pending_for_replay(budget)
    }

    /// Takes one recovery-ordered dispatch and records actual redispatches.
    pub fn dispatch_next(&mut self) -> Result<DispatchQueueOutcome, HostRejection> {
        let outcome = self.kernel.pop_next_dispatch()?;
        if let DispatchQueueOutcome::Replay(frame) = &outcome {
            self.dispatched.push(frame.clone());
        }
        Ok(outcome)
    }

    /// Supplies explicit evidence for one held probe-required invocation.
    pub fn resolve_probe(
        &mut self,
        request_id: &RequestId,
        resolution: ProbeResolution,
        max_attempts: u8,
    ) -> Result<ProbeResolutionOutcome, HostRejection> {
        self.kernel
            .resolve_probe(request_id, resolution, max_attempts)
    }

    /// Records one terminal public response.
    pub fn complete(
        &mut self,
        response: &FramedMessage,
    ) -> Result<CompletedPendingRequest, HostRejection> {
        self.kernel.complete_response(response)
    }
}

/// Reads an append-only JSONL file into typed records.
pub fn read_json_lines<T>(path: &Path) -> io::Result<Vec<T>>
where
    T: DeserializeOwned,
{
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let parsed = serde_json::from_str::<T>(line.as_str()).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid JSONL test record: {error}"),
            )
        })?;
        records.push(parsed);
    }
    Ok(records)
}

/// Assertion failure for projection doctrine checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionAssertion {
    path: String,
    message: String,
}

impl ProjectionAssertion {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProjectionAssertion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for ProjectionAssertion {}

/// Asserts that a projection obeys the doctrine implied by its surface policy.
pub fn assert_projection_doctrine<T>(projection: &T) -> Result<(), ProjectionAssertion>
where
    T: ToolProjection,
{
    let _concise = projection
        .structured_projection(DetailLevel::Concise)
        .map_err(|error| ProjectionAssertion::new("$concise", error.to_string()))?;
    let _full = projection
        .structured_projection(DetailLevel::Full)
        .map_err(|error| ProjectionAssertion::new("$full", error.to_string()))?;
    Ok(())
}
