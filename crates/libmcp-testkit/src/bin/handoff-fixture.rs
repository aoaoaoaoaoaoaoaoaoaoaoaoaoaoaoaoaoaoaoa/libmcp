//! Process fixture for two-phase release handoff tests.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use libmcp::{
    HandoffOutcome, LIBMCP_HANDOFF_SOCKET_ENV, ReleaseRuntime, load_snapshot_file_from_env,
    write_snapshot_file,
};
use libmcp_testkit as _;
use serde as _;
use serde_json::json;
use tempfile as _;

const SERVER: &str = "handoff-fixture";
const STATE_ENV: &str = "LIBMCP_TEST_HANDOFF_STATE";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let successor = std::env::var_os(LIBMCP_HANDOFF_SOCKET_ENV).is_some();
    if successor && std::env::var_os("LIBMCP_TEST_HANDOFF_FAIL").is_some() {
        return Err(io::Error::other("injected successor initialization failure").into());
    }

    let mut release = ReleaseRuntime::discover(SERVER)?;
    if successor {
        let snapshot = load_snapshot_file_from_env::<serde_json::Value>(STATE_ENV, 1024)?
            .ok_or_else(|| io::Error::other("successor received no snapshot"))?;
        if snapshot != json!({"session": "preserved"}) {
            return Err(io::Error::other("successor received the wrong snapshot").into());
        }
        release.admit_successor()?;
        fs::write(path_env("LIBMCP_TEST_CHILD_MARKER")?, b"live\n")?;
        return Ok(());
    }

    if !release.observe()?.rollout_ready() {
        return Err(io::Error::other("fixture found no selected successor").into());
    }
    let capsule = write_snapshot_file(
        "libmcp-handoff-fixture",
        &json!({
            "session": "preserved"
        }),
    )?;
    let outcome = release.handoff(STATE_ENV, capsule.path(), Duration::from_secs(5));
    let result = match outcome {
        Ok(HandoffOutcome::Relinquish) => "relinquish\n".to_owned(),
        Ok(HandoffOutcome::Retained) => "retained:no-successor\n".to_owned(),
        Err(error) => format!("retained:{error}\n"),
    };
    fs::write(path_env("LIBMCP_TEST_PARENT_MARKER")?, result)?;
    Ok(())
}

fn path_env(name: &str) -> io::Result<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{name} is unset")))
}
