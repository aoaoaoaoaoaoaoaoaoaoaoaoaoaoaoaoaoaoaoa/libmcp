# libmcp

Industrial MCP hardening spine.

`libmcp` is the shared operational substrate extracted from long-lived MCP
servers. It owns:

- typed replay and fault contracts
- JSON-RPC frame and request identity helpers
- model-facing rendering doctrine, especially porcelain-by-default output
- normalization utilities for model input friction
- standard health and telemetry payloads
- JSONL operational telemetry
- hardening test support

This repository is also the canonical owner of the `$mcp-bootstrap` Codex
skill. The installed skill should be a symlink into this repository so the skill
version tracks the library version and doctrine.

## Status

`libmcp` `1.0.0` is locked against a clean integration with
`adequate_rust_mcp`.

`fidget_spinner` is intentionally not part of `1.0.0`; it will be revisited
later once its transport shape is settled.

## Layout

- `docs/spec.md`: normative design and versioning contract
- `crates/libmcp`: public library crate
- `crates/libmcp-testkit`: shared hardening fixtures and assertions
- `assets/codex-skills/mcp-bootstrap`: canonical skill source
- `scripts/link-codex-skills`: installs the repo-owned skill into `~/.codex`
