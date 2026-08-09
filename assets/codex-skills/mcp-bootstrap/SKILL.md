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

- a stable host owns the public MCP transport, event-exact session state,
  request identities, execution knowledge, rollout, and user-facing faults
- a disposable worker owns fragile runtime dependencies and tool execution
- each routed invocation receives an explicit replay contract before dispatch
- process recovery and request replay remain orthogonal decisions
- queued client work is not dispatch-authorized work
- managed release channels remain optional; direct binary execution stays valid
- live handoff admits a successor only after private hydration and readiness
- health, telemetry, and recovery tests land before feature sprawl
- nontrivial tools default to `render=porcelain`
- rendering is exclusive: porcelain results omit `structuredContent`, while
  structured results do not repeat JSON in text content
- porcelain output should avoid JSON unless the underlying data is irreducibly
  tree-shaped; tabular data should render as compact text tables with a header
  row, `|` separators, and unquoted scalar cells
- `render` and `detail` stay separate
- structured JSON remains available for exact consumers

## Retrofit

Read:

- [references/bootstrap-retrofit.md](references/bootstrap-retrofit.md)
- [references/checklist.md](references/checklist.md)

Retrofitting order:

- separate the public session, host epoch, and worker generation
- define execution knowledge, invocation replay contracts, and typed faults
  before adding retries
- replace ad hoc dumps with porcelain-by-default output
- replace pretty-printed JSON with intentional text renderers; use compact
  table renderers for rows and reserve JSON-shaped porcelain for cases where no
  flatter representation is honest
- prefer `libmcp` projection traits, derive macros, and generic porcelain
  fallback before inventing consumer-local presentation glue
- make `render` and `detail=concise|full` real before adding extra verbosity
  knobs
- add rollout, telemetry, and recovery tests before claiming stability

## Guardrails

- Prefer a host/worker split for long-lived MCPs.
- Never auto-replay side-effecting requests unless safety is explicit in code.
- Never treat worker restart or a fault hint as replay authority.
- Never synthesize public initialization events the client did not send.
- Never make an eager builder, launcher, channel, or daemon a prerequisite for
  direct execution of a consumer binary.
- Re-open the reference docs when details matter; do not rely on memory.
- Treat this skill as a pointer to `libmcp` patterns and docs, not an
  independent spec.

This skill is intentionally thin. The reference docs contain the concrete
implementation guidance.
