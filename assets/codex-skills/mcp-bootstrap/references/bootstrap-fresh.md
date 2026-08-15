# Fresh Bootstrap

Use this when the MCP can still adopt the hard posture directly.

## 1. Ordinary server, generic host

Write one ordinary full MCP executable. It must run directly from a shell.
Managed deployment wraps that same executable with `libmcp::run_supervised`;
the business project does not grow a second protocol or supervisor.

- The host owns the public MCP transport, event-exact initialization state,
  request identities, execution knowledge, rollout, and user-facing faults.
- The worker owns backend runtimes, backend-specific retries, and tool
  execution.
- Keep the public session, host process epoch, and worker generation distinct.
- Use the generic supervisor unless a different public transport is an explicit
  product requirement. Custom transports compose the host-session kernel.

If the worker dies, the session should survive.

## 2. Effects as a typed contract

Declare private `io.libmcp/effect` metadata where standard annotations are not
enough. Recovery has four forms:

- `ReplaySafe`
- `Deduplicated(k)`
- `ProbeRequired`
- `AtMostOnce`

Session state is independently `Stateless`, `Journaled(key)`,
`Checkpointed(version)`, or `GenerationPinned`. The generic supervisor restores
journaled transitions and conservatively pins checkpointed state until a
checkpoint adapter exists.

Track whether it is `NotDispatched`, `InFlight`, `Completed`, or
`OutcomeUnknown`. A first dispatch is not a replay; queueing does not consume a
replay attempt; actual redispatch does.

Do not add blanket retry logic. Without a domain probe adapter, the generic
supervisor gives `ProbeRequired` the stricter at-most-once treatment. Unknown
`AtMostOnce` outcomes terminate with an explicit ambiguity error.

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
Treat rendering as an exclusive choice: porcelain omits `structuredContent`,
and JSON is not duplicated into text content.

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
- bounded event telemetry with intact concurrent JSONL records and an explicit
  flush policy

Do this before feature sprawl, not after the first outage.

## 7. Resource custody

- acquire temporary trees through `libmcp_testkit::TestCell` in consumer tests
- bind every child and process group to an owner that kills and waits on drop
- keep runtime scratch inside one private RAII cell; transfer only intentional
  diagnostics into bounded durable state
- pair RAII with the service manager or a startup reaper for process-death
  residue
- give telemetry, logs, caches, build cells, and immutable generations explicit
  count, age, or byte ceilings

## 8. Test the failure posture

Apply `$unit-test-doctrine` to any unit-test layer, then establish the smallest
overall basis that rejects the material failures below. These are a risk
vocabulary, not required test cases; one state-machine, model, or integration
witness may own several rows.

Cover, where credible:

- crash recovery
- worker loss before dispatch, during execution, and after completion
- replay legality, probe barriers, attempt accounting, and capacity exhaustion
- exact public and private initialization interleavings
- invalid candidates, in-flight rollover, crash recovery, and restart churn
- model-facing output shaping
- routing correctness where the backend is root-sensitive
