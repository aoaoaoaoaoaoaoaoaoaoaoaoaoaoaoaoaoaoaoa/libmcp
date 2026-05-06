# Fresh Bootstrap

Use this when the MCP can still adopt the hard posture directly.

## 1. Durable host, disposable worker

Long-lived MCPs should separate durable session ownership from fragile backend
execution.

- The host owns the MCP transport, initialization state, request IDs, replay
  policy, rollout, and user-facing error shaping.
- The worker owns backend runtimes, backend-specific retries, and tool
  execution.
- Use `libmcp`'s host-session kernel and snapshot-file handoff instead of
  rolling custom initialize seed and reexec glue.

If the worker dies, the session should survive.

## 2. Replay as a typed contract

Every request surface needs an explicit replay class:

- `Convergent`
- `ProbeRequired`
- `NeverReplay`

Do not add blanket retry or replay logic. The replay class belongs in code, not
in scattered comments.

## 3. Typed faults

Represent failures as operational faults with recovery semantics.

Baseline classes:

- transport
- process
- protocol
- timeout
- downstream response
- resource
- replay exhaustion
- rollout disruption
- invariant breach

Faults should flow through health, telemetry, and user-facing shaping.

## 4. Porcelain by default

Nontrivial tools should default to `render=porcelain`.

`render` and detail are separate axes.

- `render=porcelain|json`
- `detail=concise|full`

Porcelain should be:

- line-oriented
- deterministic
- bounded
- summary-first
- shape-aware

Structured `render=json` should remain available.

`json + concise` should be a structured summary, not merely the full payload in
different clothes.

Use library projection traits, derive macros, and rendering helpers where
possible. Do not default to pretty-printed JSON dumps and call that porcelain.

Porcelain is not JSON with fewer braces. Avoid JSON in porcelain unless the
result is irreducibly tree-shaped. When a result is tabular, render an honest
compact table: one header row, `|` separators, no quotes around ordinary string
cells, and no decorative box drawing. Token efficiency is part of the contract.

## 5. Boundary normalization

Normalize model-facing input where it is clearly safe:

- field aliases
- integer-like strings
- `file://` URIs
- stable path style controls

The goal is to eliminate trivial friction, not to hide real ambiguity.

## 6. Health and telemetry

Ship explicit operational tooling:

- health snapshot
- telemetry snapshot
- append-only event telemetry

Do this before feature sprawl, not after the first outage.

## 7. Test the failure posture

Build fake runtimes and integration tests that exercise:

- crash recovery
- replay legality
- rollout or restart churn
- model-facing output shaping
- routing correctness where the backend is root-sensitive
