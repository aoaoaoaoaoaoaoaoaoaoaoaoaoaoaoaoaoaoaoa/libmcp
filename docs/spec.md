# libmcp Spec

## Status

This document is the normative specification for `libmcp`
`1.1.0`.

`libmcp` is the reusable operational spine for hardened MCP servers. It is not
an application framework, a domain schema repository, or a mandate that every
MCP use one transport topology.

## Problem Statement

Several MCPs now share the same operational posture:

- stable public host
- disposable worker
- explicit replay contracts
- blue/green rollout and hot reinstall
- health and telemetry as first-class operational surfaces
- recovery tests for process churn and replay safety
- model-facing porcelain output instead of backend dumps

This library exists to make that posture reusable and versioned.

## Scope

`libmcp` owns the shared control plane:

- replay contract vocabulary
- typed fault taxonomy
- request identity and JSON-RPC frame helpers
- durable public-session kernel primitives
- host self-reexec snapshot file handoff
- health and telemetry base schemas
- JSONL telemetry support
- model UX doctrine primitives
- normalization helpers
- hardening test support
- canonical `$mcp-bootstrap` skill

`libmcp` explicitly does not own:

- domain tools
- backend-specific request routing
- backend-specific warm-up heuristics
- a mandatory public transport shape
- an obligation that every tool batch or support preview modes

## Durable Host Session Kernel

The library owns a reusable public-session kernel for hardened hosts.

That kernel must support:

- initialize and initialized seed capture
- pending public request journaling
- replay-attempt accounting
- queued-frame surgery after worker failure
- serialized snapshot and restore for host self-reexec

This kernel is transport-adjacent, not transport-prescriptive.

- It is shared across public MCP hosts.
- It does not force one worker bridge shape.
- Consumers may still keep worker transport logic local.

## Supported Topologies

The library must support both of these shapes:

1. stable host owning the public MCP session and talking to a private worker RPC
2. stable host owning the public MCP session and proxying to a worker MCP server

The invariants are standard. The wire shape is not.

## Core Contracts

### Replay

Every request surface must carry an explicit replay contract.

The shared vocabulary is:

- `Convergent`
- `ProbeRequired`
- `NeverReplay`

`Convergent` means repeated execution is safe because it converges on the same
observable result.

`ProbeRequired` means the host may only replay after proving that a previous
attempt did not already take effect or after reconstructing enough state to make
the replay safe.

`NeverReplay` means the request must fail rather than run again automatically.

### Faults

Failures must be represented as typed operational faults, not merely as text.

The baseline taxonomy must distinguish at least:

- transport failure
- process failure
- protocol failure
- timeout
- downstream response failure
- resource exhaustion
- replay exhaustion
- rollout disruption
- invariant breach

Each fault must carry enough structure to drive:

- retry or restart policy
- health reporting
- telemetry aggregation
- model-facing shaping

### Health

Every `libmcp` consumer should be able to expose a common operational health
core.

The base health payload includes:

- lifecycle state
- generation
- uptime
- consecutive failures
- restart count
- rollout state
- last fault

Consumers may extend it with domain-specific fields.

### Telemetry

Every `libmcp` consumer should be able to expose a common operational telemetry
core and emit append-only event telemetry.

The base telemetry payload includes:

- request counts
- success and error counts
- retry counts
- per-method aggregates
- last method error
- recent restart-triggering fault

The JSONL event path is part of the support surface because postmortem analysis
of hot paths and error concentrations is a first-class operational need.

## Model UX Doctrine

The library contract includes model-facing behavior.

### Porcelain by Default

Nontrivial tools should default to `render=porcelain`.

Structured `render=json` must remain available, but it is opt-in unless a tool
is intrinsically structured and tiny.

Porcelain output should be:

- line-oriented
- deterministic
- bounded
- summary-first
- suitable for long-running agent loops

The library should therefore provide reusable primitives for:

- render mode selection
- bounded/truncated text shaping
- stable note emission
- path rendering
- common porcelain patterns
- generic JSON-to-porcelain projection for consumers that have not yet earned
  bespoke renderers

`libmcp` does not require a universal detail taxonomy like
`summary|compact|full`. Consumers may add extra detail controls when their tool
surface actually needs them.

### Normalization

The library should reduce trivial model-facing friction where the semantics are
still unambiguous.

Examples:

- field aliases
- integer-like strings
- `file://` URIs
- consistent path-style normalization
- method-token normalization where tools have canonical names and common aliases

### Failure Shaping

Transient operational failures should be surfaced in a way that helps a model
recover:

- concise message
- typed category underneath
- retry hint when retry is plausible
- no raw backend spew by default

### Optional Nice-to-Haves

These are good patterns but are not part of the minimum contract:

- `dry_run` or preview modes
- batching for tools that do not naturally batch
- backend-specific uncertainty notes

## Canonical Skill Ownership

This repository is the canonical owner of `$mcp-bootstrap`.

The skill is versioned library collateral, not an external convenience file.
Maintaining it is part of the `libmcp` contract.

The rule is:

- the source of truth lives in this repository
- local Codex installation should point at it by symlink
- the skill must stay aligned with the current public doctrine of the library

## Versioning

`libmcp` `1.0.0` is defined by the first steady integration into
`adequate_rust_mcp`.

The `1.0.0` contract means:

- the extracted seam proved real in `adequate_rust_mcp`
- the skill and spec align with the implementation
- the model UX doctrine is embodied in helpers and tests, not just prose
- the core contracts are stable enough to be depended on directly

`libmcp` `1.1.0` adds:

- reusable host-session kernel extraction from `adequate_rust_mcp`
- generic snapshot-file handoff for host self-reexec
- generic JSON-to-porcelain rendering support
- first retrofit of `fidget_spinner` onto the shared kernel and render doctrine

`1.1.0` does not claim the final runtime-adapter release.
The deeper client/server infra lift and any runtime adapter crates remain
future work.

## Immediate Implementation Sequence

1. Create the library workspace and migrate the design note into this spec.
2. Implement the MVP core:
   - replay contracts
   - fault taxonomy
   - request identity and JSON-RPC helpers
   - health and telemetry base types
   - telemetry log support
   - render and normalization helpers
3. Port `adequate_rust_mcp` onto the shared pieces and stabilize.
4. Lock `libmcp` to `1.0.0`.
5. Revisit deeper runtime lifting only after the first consumer is truly clean.
