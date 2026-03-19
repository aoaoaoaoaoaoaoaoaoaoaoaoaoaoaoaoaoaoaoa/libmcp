//! Shared test helpers for `libmcp` consumers.

use serde::de::DeserializeOwned;
use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    path::Path,
};

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
