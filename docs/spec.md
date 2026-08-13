# libmcp Spec

## Status

This document defines the normative contract for `libmcp` `2.1.0`. Every
library-owned `MUST` is a release gate backed by executable conformance
evidence; consumer-owned `MUST` statements define the boundary the library
cannot cross without domain or transport knowledge.

`libmcp` is the reusable operational spine for hardened MCP servers. It is not
an application framework, a domain schema repository, or a mandate that every
MCP use one transport topology.

The key words `MUST`, `MUST NOT`, `SHOULD`, `SHOULD NOT`, and `MAY` distinguish
four kinds of statement:

- a library safety guarantee is a `MUST` in a library-owned contract section,
  enforced by implementation and conformance tests
- a provided mechanism is library machinery whose correct composition may
  still be a consumer obligation
- a consumer obligation is a `MUST` in the Consumer Obligations section for
  work the library cannot perform without domain or transport knowledge
- doctrine is a `SHOULD`: the preferred model-facing posture, not a safety
  claim unless a specific helper enforces it

## Purpose

Several MCPs share the same operational posture:

- stable public host
- disposable worker
- explicit replay contracts
- blue/green rollout and coordinated hot reinstall
- health and telemetry as first-class operational surfaces
- recovery tests for process churn and replay safety
- model-facing porcelain output instead of backend dumps

This library exists to make that posture reusable, exact, and versioned.

## Scope

`libmcp` contains two related planes.

The continuity plane owns shared vocabulary and mechanisms for:

- request identity and JSON-RPC frame handling
- public-session continuity across worker churn
- execution knowledge and replay contracts
- typed operational faults and recovery decisions
- coordinated readiness-gated host handoff
- health and telemetry base schemas
- append-only event telemetry
- recovery conformance tests

The presentation plane owns shared vocabulary and mechanisms for:

- render and detail selection
- explicit model-facing projections
- deterministic bounded porcelain rendering
- safe input normalization
- projection doctrine tests

The repository also owns the canonical `$mcp-bootstrap` skill.

`libmcp` explicitly does not own:

- domain tools or domain schemas
- backend-specific request routing or warm-up heuristics
- the public or private transport implementation
- domain-specific probes for uncertain side effects
- eager building, process supervision, and release publication orchestration
- long-term telemetry retention policy
- crash-consistent session persistence
- an obligation that every tool batch or support preview modes

## Continuity Boundary

The stable host owns the public MCP transport and public session. A disposable
worker owns fragile runtime dependencies and tool execution.

The library supports both common worker shapes:

1. a stable host talking to a private worker RPC
2. a stable host proxying to a worker MCP server

The invariants are shared; the worker wire shape is not.

In this specification, continuity means survival of worker replacement and
coordinated host handoff. It does not mean survival of host crashes, machine
loss, public transport loss, or uncoordinated process termination. Snapshot
handoff is not a durable persistence protocol.

## Optional Release Plane

Release management is an adapter around a standalone MCP executable, never a
prerequisite of that executable. A consumer with no managed-release environment
MUST retain its direct invocation contract and MUST NOT require a registry,
daemon, channel, or release manifest.

A managed release is immutable and carries one release identity, executable
digest, source and toolchain provenance, default argument vector, and explicit
state compatibility contract. A mutable channel selects exactly one immutable
manifest through atomic file replacement. Selection MUST NOT expose a partially
written manifest or executable.

Before a live handoff, the incumbent MUST stop at a complete-frame boundary and
MUST NOT relinquish authority while its frame reader owns partial or read-ahead
bytes. The successor receives the consumer-owned bounded snapshot through the
existing one-shot capsule mechanism, initializes privately, and reports ready
over a private barrier. Failure before activation leaves the incumbent
authoritative. After activation acknowledgement, the incumbent MUST stop
reading the public stream immediately.

Stateful releases declare the epochs they can read and the single epoch they
write. Promotion is lawful only when the successor reads the incumbent's write
epoch; rollback applies the same rule in reverse. A process switch does not
make an irreversible state migration rollback-safe.

The following concepts are distinct:

- the **public session** spans one public transport association and its
  initialization state, surviving worker replacement and coordinated host
  reexec
- a **host process epoch** is one operating-system host process lifetime and
  ends at reexec
- a **worker generation** identifies one worker incarnation within the public
  session
- the **worker handshake phase** records whether that generation has been
  initialized independently of public-session initialization
- an **invocation** is one routed public request together with its immutable
  identity, payload, and replay contract

## Execution Knowledge

Replay safety depends on what the host knows about execution as well as on the
invocation's replay contract.

For recovery purposes, an invocation occupies exactly one execution state:

- `NotDispatched`: it has definitely not reached a worker
- `InFlight`: it was dispatched and no terminal outcome has been observed
- `Completed`: one terminal outcome has been observed
- `OutcomeUnknown`: worker loss or an equivalent fault occurred after dispatch,
  so effects may have happened although no terminal outcome was observed

The first dispatch from `NotDispatched` is not a replay. A replay is a dispatch
after `OutcomeUnknown`. Scheduling or queueing a replay does not consume a
replay attempt; actual redispatch does.

## Replay Contracts

Every routed public request invocation MUST receive an explicit replay contract
before first worker dispatch. The contract belongs to the invocation after
routing, not merely to the coarse JSON-RPC method: tool arguments or recovered
domain state may affect replay legality.

The shared vocabulary remains:

- `Convergent`: after an arbitrarily completed prior attempt, another execution
  is safe without additional observation; its externally visible effects are
  observationally equivalent to one successful execution
- `ProbeRequired`: after `OutcomeUnknown`, the invocation MUST remain held until
  consumer-supplied evidence proves that it is already complete or safe to
  dispatch again
- `NeverReplay`: after `OutcomeUnknown`, the invocation MUST terminate with an
  explicit ambiguous-outcome fault rather than run again automatically

A worker restart never authorizes request replay. A retry or recovery directive
never overrides the replay contract. The library need not know how a
domain-specific probe obtains evidence, but the kernel MUST require an explicit
consumer decision before a `ProbeRequired` invocation leaves its held state.

Notifications and initialization traffic require explicit protocol-specific
handling; absence of a public request ID is not evidence that replay is safe.

## Recovery Decisions

Process recovery and request disposition are orthogonal decisions.

Process policy decides whether to continue, restart a worker, roll forward, or
abort the host. Request policy independently decides whether to first-dispatch,
replay, hold for a probe, fail with ambiguous outcome, or complete.

An implementation MAY encode these decisions together for convenience, but it
MUST preserve their independence and MUST NOT let a process action confer
request replay authority.

Faults describe operational evidence. Recovery policy combines that evidence
with execution knowledge, replay contract, current phase, and remaining budget.
The baseline fault taxonomy distinguishes at least:

- transport failure
- process failure
- protocol failure
- timeout
- downstream response failure
- resource exhaustion
- replay exhaustion
- rollout disruption
- ambiguous execution outcome
- invariant breach

Every fault MUST carry its worker generation, broad class, stable machine code,
and human diagnostic detail. A recovery hint MAY be present, but it is advisory
and remains subordinate to replay safety. Consumers own model-facing shaping.

## Kernel Laws

The continuity kernel MUST satisfy these laws:

### No Unauthorized Replay

The kernel MUST NOT redispatch an `OutcomeUnknown` invocation unless its replay
contract and any required consumer evidence authorize redispatch.

### Single Terminal Outcome

One invocation MUST produce at most one terminal public response. Reuse of an
outstanding public request ID MUST be rejected as a protocol or invariant fault
rather than overwrite existing state; an ID MAY identify a new invocation only
after its prior invocation is terminal.

### Identity Preservation

The public request ID, method, payload, replay contract, ordering sequence, and
telemetry identity of an invocation MUST describe the same immutable request.
Representations that can diverge MUST be rejected before dispatch or restore.

### Ordered Recovery

Replay-authorized invocations MUST retain their original dispatch order.
Frames accepted while recovery is in progress MUST follow the recovered work
unless a consumer declares and tests a stricter protocol-specific ordering
rule.

### Bounded Recovery

Queued frames, pending invocations, and replay attempts MUST have explicit
limits. Capacity exhaustion MUST deterministically reject work without silently
dropping or overwriting another invocation.

### Atomic Restoration

Snapshot restoration MUST either produce one fully validated kernel state or
fail without partially hydrating live state.

### Public-Session Isolation

Worker churn MUST NOT mutate public initialization state except through defined
public-session transitions. Each worker generation performs its own handshake
without pretending that the public client sent notifications it has not sent.

## Kernel Mechanisms

The reusable kernel provides mechanisms for:

- initialize and initialized seed capture
- pending invocation journaling
- execution-state and replay-attempt accounting
- bounded recovery queue surgery
- deterministic recovery decisions
- serialized snapshot and restore for coordinated host reexec

The kernel is transport-adjacent, not transport-prescriptive. Consumers own
actual reads, writes, worker processes, probes, and rollout effects.

## Reexec Snapshots

A snapshot is a confidential, version-tagged, one-shot reexec capsule. It is
not a public persistence format.

The initial compatibility promise is exact-version handoff: producer and
consumer MUST use the same snapshot format version. A successor MUST reject an
unknown version rather than guess at compatibility.

Snapshot creation and restoration MUST satisfy these requirements:

- the snapshot is fully written before its path is published to a successor
- file creation does not overwrite or follow an attacker-selected existing path
- access is restricted to the current user where the platform supports it
- restore validates frame identity, duplicate IDs, replay counters, ordering
  sequences, bounds, and phase consistency
- decode or validation failure does not produce a partially restored kernel
- cleanup removes only the capsule owned by this handoff

## Health Semantics

The common health snapshot is the stable host's view of one public session.
Consumers MAY add domain-specific fields, but the shared fields have fixed
scope:

- `uptime_ms` is host-process uptime and resets after reexec
- `generation` is the active worker generation within the public session and
  MUST NOT decrease across coordinated reexec
- `restart_count` counts worker replacements within the public session and
  survives coordinated reexec
- `consecutive_failures` counts consecutive recovery-triggering faults since
  the last successful terminal public request
- `rollout` is host rollout state
- `last_fault` is the most recent operational fault observed in the public
  session

Lifecycle state and worker handshake phase are related but not synonymous. A
host MAY remain responsive and accept bounded queued traffic while a replacement
worker is still starting. The consumer supplies those observed runtime facts and
MUST NOT collapse them into a false `Ready`.

## Telemetry Semantics

The common telemetry snapshot describes the current public session:

- request, success, error, and retry totals survive worker replacement and
  coordinated host reexec
- per-method aggregates use canonical method identity and deterministic order
- a retry is counted only when an invocation is actually redispatched
- transport/process faults remain distinct from downstream response errors
- the most recent restart-triggering fault is retained explicitly

Append-only JSONL telemetry is a support surface, not the source of truth for
live recovery state. Each event MUST be emitted as one intact JSONL record;
concurrent writers MUST NOT interleave record bytes. The library does not
promise that an event has reached stable storage unless a consumer selects an
explicit flush policy. Rotation and retention remain consumer obligations.

## Presentation Guarantees

`render` and `detail` are orthogonal:

- `render=porcelain|json` selects text or structured output
- `detail=concise|full` selects summary or expanded projection

The selected render is exclusive at the model boundary. A porcelain result
MUST NOT also carry the same projection in `structuredContent`; a JSON result
MUST NOT repeat its structured projection as serialized JSON text. Compatibility
fallbacks belong at a client-specific transport adapter, never in the default
model-facing result. Otherwise every projection consumes context twice and
`render` ceases to select anything.

Library render helpers that declare bounds MUST enforce them deterministically
and MUST mark truncation explicitly. They MUST escape or otherwise delimit
scalar text unambiguously.

Projection policy declared as enforced MUST be checked before model-facing
output is returned. A policy flag that only documents an aspiration MUST NOT be
represented as an enforced guarantee.

`json + full` is the authoritative model-facing projection. It is not
necessarily a lossless serialization of the backing domain or storage object.

Normalization MUST preserve the unambiguous value represented by valid input
or return an error. It MUST NOT silently round, saturate, or reinterpret a value
as a different identifier, number, path, or method.

## Consumer Obligations

Consumers MUST:

- assign a replay contract after domain routing and before dispatch
- implement and explicitly report evidence for `ProbeRequired` invocations
- own public and worker transport effects
- define worker startup, warm-up, and rollout policy
- preserve session-scoped health and telemetry state across coordinated reexec
- report host lifecycle and worker handshake phase as separate observed facts
- shape domain faults without exposing raw backend spew by default
- define actual concise and full model-facing projections
- emit exactly one model-facing representation of each successful projection
- protect sensitive request and telemetry content according to local policy

Consumers MUST NOT infer replay safety merely from successful worker restart,
request shape, HTTP or JSON-RPC method name, or absence of an observed response.

## Model UX Doctrine

Nontrivial tools SHOULD default to `porcelain + concise`. Structured JSON SHOULD
remain available for exact consumers.

Porcelain SHOULD be:

- line-oriented
- deterministic
- bounded
- summary-first
- suitable for long-running agent loops

The intended happy path is:

1. define a model-facing projection
2. declare and enforce its surface policy
3. render porcelain or structured JSON from that projection

Generic JSON projection is an explicit escape hatch. Tabular results SHOULD use
a compact header row, `|` separators, and unquoted ordinary scalar cells rather
than JSON-shaped text.

Operational failures SHOULD expose a concise message, typed category, and retry
hint when retry is plausible. Raw backend diagnostics SHOULD remain out of the
default model-facing projection.

Optional local conveniences remain outside the minimum contract:

- `dry_run` or preview modes
- batching for tools that do not naturally batch
- backend-specific uncertainty notes
- richer detail taxonomies beyond `concise|full`

## Conformance

The executable conformance basis MUST collectively reject violations of every
library-owned `MUST`. A stronger witness MAY own several laws; a requirement
does not demand a bespoke test when cheaper or stronger evidence already owns
its credible violations. The minimum risk matrix covers:

- every execution-knowledge and replay-contract transition
- worker failure before dispatch, during execution, and after response
- duplicate request IDs and divergent frame identity
- replay ordering, capacity exhaustion, and attempt accounting
- worker initialization and public initialization interleavings
- snapshot corruption, incompatibility, and all-or-nothing restore
- coordinated host reexec
- concurrent telemetry emission
- bounded and unambiguous rendering
- downstream derive-macro use, including generics and malformed attributes

The testkit SHOULD expose reusable assertions and fake runtimes for consumers.
Passing unit tests internal to `libmcp` is not sufficient evidence of transport
or recovery conformance. Any unit-test witnesses follow the house Unit Test
Doctrine; one-to-one requirement mapping does not confer permanence on them.

## Canonical Skill Ownership

This repository is the canonical owner of `$mcp-bootstrap`. The skill is
versioned library collateral, not an external convenience file.

The source of truth lives in this repository, local Codex installation SHOULD
point at it by symlink, and the skill MUST stay aligned with the released public
contract.

## Versioning

One release version identifies one source, contract, changelog entry, and
published artifact set. Post-tag public additions require a new version; a
release description MUST NOT be rewritten to describe code absent from its tag.

Rust API compatibility is only part of the versioning contract:

- safety guarantees MAY become stricter in a patch release but MUST NOT weaken
- public serialized schemas MAY add optional fields within a major version;
  removing or changing fields, adding required fields, or extending closed
  enums requires the compatibility treatment of a breaking change
- snapshot compatibility follows its separately declared exact-version policy
- derive-macro expansion and accepted attributes are public API
- replay meanings and kernel laws MUST NOT weaken within a major version

`libmcp` `1.0.0` established the shared replay, fault, health, telemetry,
rendering, normalization, and skill vocabulary.

The `v1.1.0` tag added host-session kernel primitives, snapshot handoff, and
generic JSON-to-porcelain rendering. Projection traits, derive macros, and later
hardening work landed after that tag and therefore belong to a subsequent
release.

The final runtime-adapter and deeper client/server lift remain future work and
are not required for conformance to this specification.
