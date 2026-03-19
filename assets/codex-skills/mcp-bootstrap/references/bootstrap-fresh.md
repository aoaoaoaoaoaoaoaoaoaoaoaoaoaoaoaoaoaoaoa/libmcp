# Fresh Bootstrap

Use this when the MCP can still adopt the hard posture directly.

## 1. Durable host, disposable worker

Long-lived MCPs should separate durable session ownership from fragile backend
execution.

- The host owns the MCP transport, initialization state, request IDs, replay
  policy, rollout, and user-facing error shaping.
- The worker owns backend runtimes, backend-specific retries, and tool
  execution.

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

Faults should flow through health, telemetry, and user-facing shaping.

## 4. Porcelain by default

Nontrivial tools should default to `render=porcelain`.

Porcelain should be:

- line-oriented
- deterministic
- bounded
- summary-first

Structured `render=json` should remain available.

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
