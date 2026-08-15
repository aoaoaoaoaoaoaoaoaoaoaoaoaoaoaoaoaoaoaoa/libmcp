//! Effect-recovery and session-migration contracts extracted from MCP catalogs.

use crate::{ReplayContract, jsonrpc::ToolName};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use thiserror::Error;

const META_KEY: &str = "io.libmcp/effect";

/// Recovery authority granted after a worker dies with an unknown outcome.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectRecovery {
    /// An arbitrary failed prefix followed by another attempt is effect-equivalent to one attempt.
    ReplaySafe,
    /// A stable key is enforced by a durable deduplication authority.
    Deduplicated,
    /// Domain evidence must resolve the unknown outcome before another attempt.
    ProbeRequired,
    /// The host may dispatch at most one attempt.
    AtMostOnce,
}

impl EffectRecovery {
    /// Projects the richer contract onto the continuity kernel's retry law.
    #[must_use]
    pub const fn replay_contract(self) -> ReplayContract {
        match self {
            Self::ReplaySafe | Self::Deduplicated => ReplayContract::Convergent,
            Self::ProbeRequired => ReplayContract::ProbeRequired,
            Self::AtMostOnce => ReplayContract::NeverReplay,
        }
    }
}

/// Logical session-state treatment during generation rollover.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionStateContract {
    /// No business-session state must cross generations.
    Stateless,
    /// Successful session-only calls compact under this key and replay before activation.
    Journaled {
        /// Stable journal compaction key.
        key: String,
    },
    /// State moves through a bounded, versioned business checkpoint.
    Checkpointed {
        /// Checkpoint format version.
        version: u64,
    },
    /// The generation cannot be replaced while this state remains live.
    GenerationPinned,
}

/// Complete host-visible contract for one MCP tool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolEffectContract {
    recovery: EffectRecovery,
    state: SessionStateContract,
    deduplication_key_pointer: Option<String>,
}

impl ToolEffectContract {
    /// Constructs an explicit tool contract.
    #[must_use]
    pub const fn new(recovery: EffectRecovery, state: SessionStateContract) -> Self {
        Self {
            recovery,
            state,
            deduplication_key_pointer: None,
        }
    }

    /// Returns the crash-recovery law.
    #[must_use]
    pub const fn recovery(&self) -> EffectRecovery {
        self.recovery
    }

    /// Returns the rollover state law.
    #[must_use]
    pub const fn state(&self) -> &SessionStateContract {
        &self.state
    }

    /// Returns the JSON Pointer naming a durable operation key.
    #[must_use]
    pub fn deduplication_key_pointer(&self) -> Option<&str> {
        self.deduplication_key_pointer.as_deref()
    }
}

/// Public tool catalog plus host-private contracts removed from its projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCatalog {
    public_tools: Vec<Value>,
    contracts: HashMap<ToolName, ToolEffectContract>,
}

impl ToolCatalog {
    /// Refines a `tools/list` result and removes the reserved private metadata.
    pub fn parse(result: &Value) -> Result<Self, EffectContractError> {
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or(EffectContractError::MissingTools)?;
        let mut public_tools = Vec::with_capacity(tools.len());
        let mut contracts = HashMap::with_capacity(tools.len());

        for raw in tools {
            let object = raw.as_object().ok_or(EffectContractError::ToolNotObject)?;
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .ok_or(EffectContractError::MissingToolName)?;
            let name = ToolName::try_new(name.to_owned())
                .map_err(|_| EffectContractError::InvalidToolName)?;
            if contracts.contains_key(&name) {
                return Err(EffectContractError::DuplicateTool(name.as_str().to_owned()));
            }

            let contract = parse_contract(object)?;
            let mut public = raw.clone();
            strip_private_contract(&mut public);
            let _previous = contracts.insert(name, contract);
            public_tools.push(public);
        }

        Ok(Self {
            public_tools,
            contracts,
        })
    }

    /// Returns the public catalog projection.
    #[must_use]
    pub fn public_tools(&self) -> &[Value] {
        &self.public_tools
    }

    /// Returns the contract for one named tool.
    #[must_use]
    pub fn contract(&self, name: &ToolName) -> Option<&ToolEffectContract> {
        self.contracts.get(name)
    }

    /// Returns a deterministic equality witness for catalog rollover.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&self.public_tools)
    }
}

/// Invalid or contradictory private effect metadata.
#[derive(Debug, Error)]
pub enum EffectContractError {
    /// The `tools/list` result omitted its tool array.
    #[error("tools/list result has no tools array")]
    MissingTools,
    /// One tool definition was not an object.
    #[error("tool definition is not an object")]
    ToolNotObject,
    /// One tool omitted its name.
    #[error("tool definition has no name")]
    MissingToolName,
    /// One tool name violated MCP token bounds.
    #[error("tool definition has an invalid name")]
    InvalidToolName,
    /// The catalog defined one tool more than once.
    #[error("tool catalog contains duplicate `{0}`")]
    DuplicateTool(String),
    /// Reserved metadata used an unknown recovery kind.
    #[error("tool `{tool}` uses unknown recovery kind `{kind}`")]
    UnknownRecovery {
        /// Tool carrying the invalid contract.
        tool: String,
        /// Unrecognized recovery value.
        kind: String,
    },
    /// Reserved metadata used malformed state data.
    #[error("tool `{tool}` has invalid state contract: {reason}")]
    InvalidState {
        /// Tool carrying the invalid contract.
        tool: String,
        /// Precise rejected field.
        reason: &'static str,
    },
}

fn parse_contract(object: &Map<String, Value>) -> Result<ToolEffectContract, EffectContractError> {
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(contract) = object
        .get("_meta")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get(META_KEY))
        .and_then(Value::as_object)
    {
        let recovery_object = contract
            .get("recovery")
            .and_then(Value::as_object)
            .ok_or_else(|| EffectContractError::UnknownRecovery {
                tool: name.to_owned(),
                kind: "<missing>".to_owned(),
            })?;
        let recovery = recovery_object
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| EffectContractError::UnknownRecovery {
                tool: name.to_owned(),
                kind: "<missing>".to_owned(),
            })?;
        let recovery = match recovery {
            "replay_safe" => EffectRecovery::ReplaySafe,
            "deduplicated" => EffectRecovery::Deduplicated,
            "probe_required" => EffectRecovery::ProbeRequired,
            "at_most_once" => EffectRecovery::AtMostOnce,
            other => {
                return Err(EffectContractError::UnknownRecovery {
                    tool: name.to_owned(),
                    kind: other.to_owned(),
                });
            }
        };
        let state = parse_state(name, contract.get("state"))?;
        if matches!(state, SessionStateContract::Journaled { .. })
            && !matches!(
                recovery,
                EffectRecovery::ReplaySafe | EffectRecovery::Deduplicated
            )
        {
            return Err(EffectContractError::InvalidState {
                tool: name.to_owned(),
                reason: "journaled transitions must be replay-safe",
            });
        }
        let deduplication_key_pointer = if recovery == EffectRecovery::Deduplicated {
            let pointer = recovery_object
                .get("key")
                .and_then(Value::as_str)
                .filter(|pointer| pointer.starts_with('/') && pointer.len() > 1)
                .ok_or_else(|| EffectContractError::InvalidState {
                    tool: name.to_owned(),
                    reason: "deduplicated recovery requires a JSON Pointer key",
                })?;
            Some(pointer.to_owned())
        } else {
            None
        };
        return Ok(ToolEffectContract {
            recovery,
            state,
            deduplication_key_pointer,
        });
    }

    let recovery = legacy_recovery(object).unwrap_or_else(|| {
        let annotations = object.get("annotations").and_then(Value::as_object);
        if annotations
            .and_then(|value| value.get("readOnlyHint"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || annotations
                .and_then(|value| value.get("idempotentHint"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            EffectRecovery::ReplaySafe
        } else {
            EffectRecovery::AtMostOnce
        }
    });
    Ok(ToolEffectContract::new(
        recovery,
        SessionStateContract::Stateless,
    ))
}

fn parse_state(
    tool: &str,
    value: Option<&Value>,
) -> Result<SessionStateContract, EffectContractError> {
    let Some(state) = value.and_then(Value::as_object) else {
        return Err(EffectContractError::InvalidState {
            tool: tool.to_owned(),
            reason: "missing state object",
        });
    };
    let Some(kind) = state.get("kind").and_then(Value::as_str) else {
        return Err(EffectContractError::InvalidState {
            tool: tool.to_owned(),
            reason: "missing state kind",
        });
    };
    match kind {
        "stateless" => Ok(SessionStateContract::Stateless),
        "journaled" => {
            let key = state
                .get("key")
                .and_then(Value::as_str)
                .filter(|key| !key.is_empty())
                .ok_or_else(|| EffectContractError::InvalidState {
                    tool: tool.to_owned(),
                    reason: "journal key is missing",
                })?;
            Ok(SessionStateContract::Journaled {
                key: key.to_owned(),
            })
        }
        "checkpointed" => {
            let version = state
                .get("version")
                .and_then(Value::as_u64)
                .ok_or_else(|| EffectContractError::InvalidState {
                    tool: tool.to_owned(),
                    reason: "checkpoint version is missing",
                })?;
            Ok(SessionStateContract::Checkpointed { version })
        }
        "generation_pinned" => Ok(SessionStateContract::GenerationPinned),
        _ => Err(EffectContractError::InvalidState {
            tool: tool.to_owned(),
            reason: "unknown state kind",
        }),
    }
}

fn legacy_recovery(object: &Map<String, Value>) -> Option<EffectRecovery> {
    let annotations = object.get("annotations")?;
    find_named_text(annotations, "replayContract").and_then(|contract| match contract {
        "convergent" => Some(EffectRecovery::ReplaySafe),
        "probe_required" => Some(EffectRecovery::ProbeRequired),
        "never_replay" => Some(EffectRecovery::AtMostOnce),
        _ => None,
    })
}

fn find_named_text<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    match value {
        Value::Object(object) => object.get(name).and_then(Value::as_str).or_else(|| {
            object
                .values()
                .find_map(|child| find_named_text(child, name))
        }),
        Value::Array(array) => array.iter().find_map(|child| find_named_text(child, name)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

fn strip_private_contract(tool: &mut Value) {
    let Some(meta) = tool.get_mut("_meta").and_then(Value::as_object_mut) else {
        return;
    };
    let _removed = meta.remove(META_KEY);
    if meta.is_empty()
        && let Some(tool) = tool.as_object_mut()
    {
        let _removed = tool.remove("_meta");
    }
}

#[cfg(test)]
mod tests {
    use super::{EffectRecovery, SessionStateContract, ToolCatalog};
    use crate::ToolName;
    use serde_json::json;

    #[test]
    fn catalog_refinement_is_conservative_and_private() {
        let result = json!({"tools": [
            {
                "name": "observe",
                "annotations": {"readOnlyHint": true}
            },
            {
                "name": "spend",
                "annotations": {
                    "readOnlyHint": true,
                    "consult": {"replayContract": "never_replay"}
                }
            },
            {
                "name": "bind",
                "_meta": {"io.libmcp/effect": {
                    "recovery": {"kind": "replay_safe"},
                    "state": {"kind": "journaled", "key": "project"}
                }}
            }
        ]});
        let catalog = ToolCatalog::parse(&result);
        assert!(catalog.is_ok());
        let Ok(catalog) = catalog else { return };

        let observe_name = ToolName::try_new("observe");
        assert!(observe_name.is_ok());
        let Ok(observe_name) = observe_name else {
            return;
        };
        let observe = catalog.contract(&observe_name);
        assert!(observe.is_some_and(|value| value.recovery() == EffectRecovery::ReplaySafe));
        let spend_name = ToolName::try_new("spend");
        assert!(spend_name.is_ok());
        let Ok(spend_name) = spend_name else { return };
        let spend = catalog.contract(&spend_name);
        assert!(spend.is_some_and(|value| value.recovery() == EffectRecovery::AtMostOnce));
        let bind_name = ToolName::try_new("bind");
        assert!(bind_name.is_ok());
        let Ok(bind_name) = bind_name else { return };
        let bind = catalog.contract(&bind_name);
        assert!(bind.is_some_and(|value| matches!(
            value.state(),
            SessionStateContract::Journaled { key } if key == "project"
        )));
        assert!(
            catalog
                .public_tools()
                .iter()
                .all(|tool| tool.get("_meta").is_none())
        );
    }
}
