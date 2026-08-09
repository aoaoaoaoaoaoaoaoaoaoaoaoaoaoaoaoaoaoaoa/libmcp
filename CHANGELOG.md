# Changelog

## Unreleased

## 2.1.0

- add a lossless timed blocking frame reader for idle host control loops; it
  retains partial and read-ahead frames and exposes the buffered-input barrier
  required before coordinated process replacement
- add an optional release-channel runtime that preserves direct standalone
  execution when managed-release environment is absent
- add immutable release manifests, executable verification, atomic channel
  selection, explicit state-epoch compatibility, and previous-release rollback
- replace blind self-exec with a private two-phase successor barrier: the new
  host must hydrate its bounded session capsule and report live before the
  incumbent relinquishes the public stream
- add process-level conformance tests proving successful continuity and
  incumbent survival after injected successor initialization failure

## 2.0.2

- add bounded blocking frame readers and writers with the same limits and
  validation as the Tokio frame I/O surface

## 2.0.1

- add the missing generic terminal-error transition to `OperationalLedger`,
  keeping host-originated failures out of downstream-response and
  recovery-error counters

## 2.0.0

Machine-grade continuity and presentation release. `docs/spec.md` is now the
normative public contract.

Included in `2.0.0`:

- event-exact public-session state separated from worker generations and
  private worker handshakes
- explicit execution knowledge, invocation-local replay contracts, probe
  barriers, ordered recovery, and actual-dispatch attempt accounting
- bounded pending and queue admission with queued client work distinguished
  from authorized replay
- sealed JSON-RPC identities, duplicate-member rejection, and bounded frame IO
- exact-version, atomically validated, private one-shot reexec capsules
- advisory process recovery hints severed from request replay authority
- session-scoped health and telemetry with terminal outcomes distinct from
  recovery-fault incidents and portable record-atomic JSONL emission
- enforced projection policies, downstream-safe derive macros, bounded
  porcelain rendering, and exact input normalization
- downstream churn and derive conformance in `libmcp-testkit`
- canonical `$mcp-bootstrap` doctrine aligned with the released contract

Breaking changes from `1.1.0` include sealed invariant-bearing fields, the
execution/replay state algebra, explicit queued-versus-replay outcomes,
advisory `RecoveryHint`, corrected telemetry counters, and a single
materialized projection-policy authority.

## 1.1.0

Additive release on top of the locked `1.0.0` foundation.

Included in `1.1.0`:

- reusable host-session kernel for initialize seed capture, pending request
  journaling, replay budgeting, and queue rebuild
- snapshot-file helpers for host self-reexec handoff
- generic JSON-to-porcelain rendering for doctrine-compliant default output
- retrofit of `adequate_rust_mcp` onto the shared host-session kernel
- first `fidget_spinner` retrofit onto the shared kernel and render doctrine

Still intentionally excluded:

- final runtime adapter crates
- a forced single worker transport shape
- the deeper client/server infra lift reserved for a later release

## 1.0.0

Initial stable release.

This release establishes `libmcp` as the reusable operational spine for
hardened MCP servers and the canonical owner of `$mcp-bootstrap`.

Included in `1.0.0`:

- replay contract vocabulary
- typed fault model
- JSON-RPC request/frame helpers
- base health and telemetry payloads
- append-only JSONL telemetry support
- model-facing render and normalization helpers
- versioned `$mcp-bootstrap` skill collateral

Explicitly excluded from `1.0.0`:

- forced runtime adapter crates
- backend-specific warm-up or routing policy beyond what the first consumer
  still keeps locally
