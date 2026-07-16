# Fresh Bootstrap

Use this when the MCP can still adopt the hard posture directly.

## 1. Durable host, disposable worker

Long-lived MCPs should separate durable session ownership from fragile backend
execution.

- The host owns the public MCP transport, event-exact initialization state,
  request identities, execution knowledge, rollout, and user-facing faults.
- The worker owns backend runtimes, backend-specific retries, and tool
  execution.
- Keep the public session, host process epoch, and worker generation distinct.
- Use `libmcp`'s host-session kernel and bounded, private, one-shot snapshot
  capsules instead of rolling custom initialize seed and reexec glue.

If the worker dies, the session should survive.

## 2. Replay as a typed contract

Every routed invocation needs an explicit replay contract before first worker
dispatch:

- `Convergent`
- `ProbeRequired`
- `NeverReplay`

Track whether it is `NotDispatched`, `InFlight`, `Completed`, or
`OutcomeUnknown`. A first dispatch is not a replay; queueing does not consume a
replay attempt; actual redispatch does.

Do not add blanket retry logic. The contract belongs to the invocation after
domain routing, not merely to a coarse JSON-RPC method. Hold `ProbeRequired`
work until explicit domain evidence arrives, and terminate `NeverReplay` work
with an ambiguous-outcome error after uncertain execution.

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
- ambiguous execution outcome
- invariant breach

Faults should flow through health, telemetry, and user-facing shaping. Any
process-recovery hint is advisory: it must never authorize request replay.

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

- health snapshots that distinguish host lifecycle from worker handshake phase
- session-scoped request, terminal outcome, recovery-fault, and retry totals
- append-only event telemetry with intact concurrent JSONL records and an
  explicit flush policy

Do this before feature sprawl, not after the first outage.

## 7. Test the failure posture

Build fake runtimes and integration tests that exercise:

- crash recovery
- worker loss before dispatch, during execution, and after completion
- replay legality, probe barriers, attempt accounting, and capacity exhaustion
- exact public and private initialization interleavings
- coordinated reexec, corrupt capsules, and version rejection
- rollout or restart churn
- model-facing output shaping
- routing correctness where the backend is root-sensitive
