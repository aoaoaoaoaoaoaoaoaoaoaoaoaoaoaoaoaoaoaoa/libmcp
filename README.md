# libmcp

Industrial MCP hardening spine.

`libmcp` is the shared operational substrate extracted from long-lived MCP
servers. It owns:

- typed replay and fault contracts
- JSON-RPC frame and request identity helpers
- durable host-session kernel and snapshot-file handoff
- model-facing rendering doctrine, especially porcelain-by-default output
- normalization utilities for model input friction
- standard health and telemetry payloads
- JSONL operational telemetry
- hardening test support

This repository is also the canonical owner of the `$mcp-bootstrap` Codex
skill. The installed skill should be a symlink into this repository so the skill
version tracks the library version and doctrine.

## Status

`libmcp` `1.1.0` builds on the locked `1.0.0` base with a reusable
host-session kernel, snapshot-file reexec handoff, a generic JSON-to-porcelain
renderer, and a first `fidget_spinner` retrofit.

The eventual runtime-adapter and deeper client/server lift remain future work;
this release does not pretend that phase is complete.

## Layout

- `docs/spec.md`: normative design and versioning contract
- `crates/libmcp`: public library crate
- `crates/libmcp-testkit`: shared hardening fixtures and assertions
- `assets/codex-skills/mcp-bootstrap`: canonical skill source
- `scripts/link-codex-skills`: installs the repo-owned skill into `~/.codex`
