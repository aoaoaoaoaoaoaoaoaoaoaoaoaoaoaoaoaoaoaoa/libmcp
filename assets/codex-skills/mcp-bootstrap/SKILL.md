---
name: mcp-bootstrap
description: Bootstrap or retrofit an industrial-grade MCP server with `libmcp` patterns. Use when creating a new MCP, hardening an existing one, or reviewing one for host/worker separation, replay safety, typed faults, porcelain output, telemetry, rollout, or recovery testing.
---

# MCP Bootstrap

Use this skill when a target MCP should follow the architecture and operational
patterns used by `libmcp`.

`libmcp` is the source of truth. This skill is a routing guide to the relevant
reference docs, not a substitute for them.

Start by classifying the target:

- Fresh bootstrap: the project can adopt the `libmcp` architecture directly.
- Retrofit: the project already has live behavior or recovery logic that must be
  tightened deliberately.

Then choose exactly one surface.

## Fresh Bootstrap

Read:

- [references/bootstrap-fresh.md](references/bootstrap-fresh.md)
- [references/checklist.md](references/checklist.md)

Default pattern:

- a stable host owns the public MCP transport, session state, request IDs,
  replay policy, rollout, and user-facing fault shaping
- a disposable worker owns fragile runtime dependencies and tool execution
- each request surface defines explicitly whether replay is safe
- health, telemetry, and recovery tests land before feature sprawl
- nontrivial tools default to `render=porcelain`
- `render` and `detail` stay separate
- structured JSON remains available for exact consumers

## Retrofit

Read:

- [references/bootstrap-retrofit.md](references/bootstrap-retrofit.md)
- [references/checklist.md](references/checklist.md)

Retrofitting order:

- separate durable transport and session ownership from fragile execution
- define replay classes and typed faults before adding retries
- replace ad hoc dumps with porcelain-by-default output
- make `render` and `detail=concise|full` real before adding extra verbosity
  knobs
- add rollout, telemetry, and recovery tests before claiming stability

## Guardrails

- Prefer a host/worker split for long-lived MCPs.
- Never auto-replay side-effecting requests unless safety is explicit in code.
- Re-open the reference docs when details matter; do not rely on memory.
- Treat this skill as a pointer to `libmcp` patterns and docs, not an
  independent spec.

This skill is intentionally thin. The reference docs contain the concrete
implementation guidance.
