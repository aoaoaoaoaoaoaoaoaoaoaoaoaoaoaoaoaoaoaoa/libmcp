---
name: mcp-bootstrap
description: Bootstrap or retrofit an industrial-grade MCP server with libmcp's host/worker posture, replay contracts, typed faults, porcelain-by-default output, telemetry, and regression tests. Use when creating a new MCP, hardening an existing one, or reviewing an MCP for long-lived session safety, hot rollout, crash recovery, model UX, or worktree/workspace correctness.
---

# MCP Bootstrap

`libmcp` is the source of truth for the hard posture of long-lived MCPs.

Use this skill when:

- bootstrapping a new MCP
- retrofitting an existing MCP onto `libmcp`
- reviewing an MCP for operational hardening or model UX doctrine

Start by classifying the target:

- Fresh bootstrap: the project can adopt the architecture directly.
- Retrofit: the project already has live behavior or ad hoc recovery logic that
  must be tightened deliberately.

## Fresh Bootstrap

Read:

- [references/bootstrap-fresh.md](references/bootstrap-fresh.md)
- [references/checklist.md](references/checklist.md)

Default posture:

- let a stable host own the public MCP transport and session
- let disposable workers own fragile runtime dependencies
- make replay legality explicit per request surface
- ship health, telemetry, and recovery tests before feature sprawl
- default nontrivial tool output to porcelain
- keep structured JSON output available where exact consumers need it

## Retrofit

Read:

- [references/bootstrap-retrofit.md](references/bootstrap-retrofit.md)
- [references/checklist.md](references/checklist.md)

Retrofit in order:

- separate durable transport ownership from fragile execution
- define typed faults and replay contracts before adding retries
- adopt the model UX doctrine, especially porcelain-by-default output
- add rollout, telemetry, and recovery tests before claiming stability

## Guardrails

- Prefer a host/worker split for any long-lived MCP.
- Do not auto-retry or auto-replay side-effecting requests without an explicit
  proof of safety.
- Treat `$mcp-bootstrap` as versioned `libmcp` collateral, not independent lore.
- Re-open the reference docs when details matter; do not rely on memory.

This skill is intentionally thin. The reference docs are the payload.
