# libmcp Formal Semantics

## Status

This document defines the abstract machine refined by the supervised MCP
runtime. It is normative for continuity, effect recovery, and live release
rollover. `docs/spec.md` states the product contract. This document gives the
state space, transition relation, laws, proof obligations, and the boundary of
what can be proved.

The notation is mathematical but deliberately small. The implementation is not
a theorem prover. Conformance consists of:

1. a representation map from concrete Rust state to this abstract state;
2. local checks that concrete transitions preserve the stated invariants;
3. adversarial trace tests at every process and acknowledgement boundary;
4. explicit consumer evidence for premises that cannot be derived by the host.

The model covers one public MCP transport. Independent transports are separate
machines.

## Vocabulary

Names are fixed as follows.

- A **session** is one public MCP transport association and its initialization
  history.
- The **host** is the stable process that alone owns the public transport.
- A **generation** is one immutable business-server release identity.
- A **worker** is one process incarnation of a generation.
- An **invocation** is one accepted public request with immutable identity,
  payload, sequence, and effect contract.
- An **attempt** is one dispatch of an invocation to one worker.
- An **effect** is any externally distinguishable change caused by business
  execution, including resource consumption and acquired obligations.
- A **terminal outcome** is one JSON-RPC result or error emitted for an
  invocation.
- **Rollover** replaces the active generation without replacing the public
  session.
- **Recovery** restores service after unplanned worker loss.
- A **fence** is the sequence boundary after which accepted work is queued for
  a successor rather than dispatched to the incumbent.
- A **checkpoint** is bounded business-session state whose successful restore
  is sufficient to continue a session on another generation.

“Replay” means a second attempt after the host can no longer determine whether
an earlier attempt took effect. Repeating a request known not to have crossed
the dispatch boundary is a first dispatch, not replay.

## Domains

Let:

- `J` be JSON-RPC request identifiers;
- `N` be invocation sequence numbers, ordered by `<`;
- `G` be immutable generation identities;
- `W` be worker incarnation identities;
- `M` be MCP method names;
- `P` be serialized JSON payloads;
- `V` be terminal JSON-RPC values;
- `Σ` be private logical session state;
- `Ω` be the external world;
- `D` be effect domains such as a filesystem tree, database, process tree,
  quota account, user, device, or remote service;
- `K` be stable operation keys;
- `Q` be probes returning domain evidence;
- `C` be bounded checkpoints.

An invocation is:

```text
i = ⟨sid, id, seq, method, payload, contract⟩
    ∈ Session × J × N × M × P × Contract
```

Within one session, `(id, seq)` is unique. `id` may be reused only after its
prior invocation is terminal; `seq` is never reused.

Business execution is a partial relation:

```text
⟦i⟧ ⊆ (Σ × Ω) × (V × Σ × Ω × L)
```

where `L` is the set of live obligations left after the response: processes,
locks, leases, subscriptions, temporary artifacts, or other resources whose
lifetime is not exhausted by returning `V`. The relation admits
nondeterminism, concurrent changes to `Ω`, worker failure, and prefixes that
produce effects without producing `V`.

`obs_D(Ω)` projects the externally distinguishable state of effect domain `D`.
Two worlds are equivalent for footprint `F ⊆ D`, written `Ω ≈_F Ω′`, when
every observer outside `F` sees the same state. Effect equivalence inside `F`
is supplied by the contract for that domain.

## Effect Contracts

The host needs one classification: what may it do after an attempt's outcome
becomes unknown?

```text
Effect ::= ReplaySafe
         | Deduplicated(k)
         | ProbeRequired(q)
         | AtMostOnce
```

This taxonomy is intentionally closed and operational. It does not attempt to
model every way software can affect the world.

`ReplaySafe` requires this law: for every reachable `(σ, ω)` and every failed
prefix `p` of an attempt, executing another complete attempt produces an effect
history equivalent to some history containing one complete attempt.

```text
p ; ⟦i⟧complete ≈effect ⟦i⟧complete
```

The law concerns effects, not byte-identical results. A mailbox read may return
newer data on retry and still be replay-safe. A nominal read that spends money,
consumes quota, sends telemetry, or acquires a resource is not replay-safe
unless those effects also satisfy the law.

`Deduplicated(k)` requires the effect authority to atomically associate stable
key `k` with one committed outcome. Every attempt carries the same key. A
host-local table is not durable evidence when the external effect can outlive
the host or worker.

`ProbeRequired(q)` permits no replay while outcome is unknown. Probe `q(i)`
returns:

```text
Completed(v) | SafeToRetry | StillUnknown
```

Only `SafeToRetry` authorizes another attempt. `Completed(v)` completes the
invocation without replay. `StillUnknown` leaves it held.

`AtMostOnce` permits at most one dispatch. Unknown outcome terminates with an
explicit ambiguity error. This is the default for unclassified mutations,
expensive inference, human elicitation, message transmission, process creation,
and one-way notification.

The authority order is:

```text
AtMostOnce ⊑ ProbeRequired ⊑ Deduplicated ⊑ ReplaySafe
```

where `a ⊑ b` means `a` grants no more automatic retry authority than `b`.
Composite operations take the lower authority unless their business protocol
proves a stronger contract for the composite.

The following ordinary cases exhaust the intended classification surface.

| Business behavior | Contract |
|---|---|
| pure computation or effect-free observation | `ReplaySafe` |
| mutation with a proved convergence law | `ReplaySafe` |
| mutation under a durable idempotency key | `Deduplicated(k)` |
| transaction with an authoritative status query | `ProbeRequired(q)` |
| irreversible, consumptive, opaque, or unclassified work | `AtMostOnce` |

Compensation is a later effect, not evidence that retry is safe. Execution
followed by compensation remains visible to concurrent observers, logs,
billing, messages, and physical systems.

### Session State

Effect recovery and session migration are separate. A server declares one
rollover state contract:

```text
State ::= Stateless
        | Journaled(key)
        | Checkpointed(version)
        | GenerationPinned
```

Thus `Contract = Effect × State`. Live obligations are tracked separately in
runtime state because they arise and terminate during execution.

A journal entry is a successful session-only transition and must have no
external effect. Its compaction key is either a declared constant or a JSON
Pointer to a scalar request field; tool identity namespaces the result. It
becomes authoritative after the host observes success and before the public
response. A checkpoint is bounded and restored atomically. Generation-pinned
state forces rollover to wait or retain the incumbent.

### Live Obligations

Processes, locks, leases, subscriptions, and background jobs that outlive a
response are **live obligations**, not additional effect kinds. Each has one
declared owner and release law. A worker-owned obligation pins that generation;
an obligation intended to survive it needs an external owner and stable
identity. The runtime applies RAII to every resource it owns. Forgetting a PID
or temporary path is not detachment.

### Nested MCP Effects

A worker-originated MCP request, including sampling, elicitation, or roots
access, is a child invocation causally owned by the business invocation that
triggered it. Its contract composes with the parent contract. The host rewrites
its request identifier bijectively and routes the response to the originating
generation.

Inactive candidates may not issue externally visible child invocations during
warm-up. Such a request rejects candidacy. Otherwise warming a candidate could
spend money, prompt a human, or mutate client state before the release becomes
authoritative.

Notifications have no terminal acknowledgement. Unless their protocol defines
a durable key or repeatability law, dispatch makes their outcome unknown and
they are `AtMostOnce`. The host does not invent reliable delivery over a
one-way method.

Progress notifications inherit the generation pin of their invocation. A
candidate emits no public progress before activation. An incumbent draining a
live invocation may continue to emit progress until that invocation is
terminal.

## Abstract Host State

The host state is:

```text
H = ⟨phase, active, candidate, workers, invocations, queue,
     init, catalog, journal, callbacks, next_seq, fence⟩
```

with:

```text
phase ::= Cold
        | Serving(g)
        | Preparing(g, h)
        | AwaitingCatalogRefresh(g, h, δ)
        | Draining(g, h, κ)
        | Activating(g, h)
        | Recovering(g)
        | Stopped

worker ::= Starting | Ready | Active | Draining | Dead

invocation ::= Accepted
             | Dispatched(g, attempt)
             | OutcomeUnknown(g, attempt)
             | HeldForProbe
             | Terminal(v)
```

`κ` is a fence sequence. `queue` is ordered by invocation sequence. `init`
contains the exact accepted initialize request and initialized notification.
`catalog` is the public projection of the active worker's catalog. `journal` is
a bounded compacted sequence of successful session-only transitions.

The host is the sole writer of public stdout. Each worker has one private MCP
pipe pair. Worker processes, pipes, reader tasks, readiness timers, and
candidate state are owned resources and are reaped on every terminal path.

## Observable Actions

The labeled transition system uses these visible actions:

```text
accept(i)             host accepts a public request
dispatch(g, i, n)     attempt n may begin executing in generation g
reply(i, v)           host emits the sole terminal outcome
notify(x)             host emits a public MCP notification
child_request(g, r)   host exposes a worker-originated request
activate(g, h)        active authority changes atomically from g to h
```

Internal actions include channel observation, executable verification, worker
spawn, private initialization, catalog fetch, checkpoint restore, journal
replay, probe resolution, and process reaping.

`dispatch` is conservatively linearized immediately before the first write is
attempted on a worker pipe. A partial or failed write therefore leaves the
attempt possibly executed. This may reject work that provably did not reach the
worker, but it never licenses an unsafe duplicate.

`activate(g, h)` is the sole generation-authority linearization point. Before
it, public invocations dispatch only to `g`; after it, only to `h`. Warm-up and
state restoration are not activation.

## Transition Rules

Side conditions omitted below are mandatory. A rule whose side condition is
false is unavailable.

### Public Initialization

```text
Cold --accept(initialize)--> Cold
     --spawn/verify g; private initialize g--> Cold
     --reply(initialize, patched_capabilities)--> Serving(g)
```

The host patches list-change capabilities it actually implements. It records
the exact public initialization seed. Each later worker receives an equivalent
private handshake with host-owned request IDs. Candidate handshake responses
never escape to the public client.

### Normal Dispatch

For a request `i` in `Serving(g)`:

```text
accept(i); classify(i); mark Dispatched(g, 0); dispatch(g, i, 0)
```

Classification precedes dispatch. Absence, contradiction, or malformed private
metadata resolves to the least replay authority consistent with standard MCP
annotations; unclassified tool calls are `AtMostOnce`.

On a valid worker response:

```text
Dispatched(g, n) --reply(i, v)--> Terminal(v)
```

The host removes `i` from pending state only as part of committing its terminal
response. A successful journaled session transition updates the journal before
the public response is emitted.

### Candidate Preparation

When the release channel names `h ≠ g`:

```text
Serving(g) --verify h; spawn h; initialize h; list h--> Preparing(g, h)
```

Preparation has a deadline and fixed resource bounds. Candidate `h` remains
inactive. Verification, handshake, readiness, state compatibility, and catalog
validation must all succeed. Any failure destroys `h` and returns to
`Serving(g)` without changing authority.

If public catalogs are identical, the host chooses fence `κ = next_seq` and
enters `Draining(g, h, κ)`. Requests with `seq ≥ κ` are accepted into the
bounded queue but not dispatched.

If catalogs differ, the host emits the appropriate MCP list-changed
notification and enters `AwaitingCatalogRefresh(g, h, δ)`, where `δ` is a
finite grace deadline. The incumbent continues serving. Receipt of the
matching public list request, or expiry of `δ`, chooses the fence and starts
draining. The notification gives a conforming client an opportunity to refresh;
client inaction cannot retain an obsolete generation indefinitely.

### Drain and Activation

In `Draining(g, h, κ)`:

- every invocation dispatched before `κ` remains pinned to `g`;
- no invocation at or after `κ` dispatches to `g`;
- cancellation is routed to the generation owning the target invocation;
- `g` is not terminated while it owns an in-flight invocation or scoped
  obligation.

When all pre-fence invocations and generation-pinned obligations are terminal:

```text
Draining(g, h, κ)
  --restore checkpoint/journal into h-->
Activating(g, h)
  --activate(g, h)-->
Serving(h)
```

If a refresh request established the fence, activation returns `h`'s public
catalog in that response. A refresh arriving during grace-expiry draining is
queued and answered after activation. Identical-catalog activation is silent.
Queued invocations then dispatch to `h` in sequence order. Only after activation
and queue transfer may `g` be terminated and reaped.

If restoration fails, `h` is destroyed, the fence is removed, and queued work
returns to `g` in order. No candidate state is merged into `g`.

### Worker Loss

If active worker `g` dies unexpectedly:

```text
Dispatched(g, n) --> OutcomeUnknown(g, n)
Serving(g) --> Recovering(g)
```

For each unknown invocation in sequence order:

```text
ReplaySafe       -> queue replay n+1
Deduplicated(k)  -> queue retry n+1 with identical k
ProbeRequired(q) -> HeldForProbe
AtMostOnce    -> reply(AmbiguousOutcome)
```

The host selects one verified generation, starts a fresh worker, replays the
initialization seed, restores the session checkpoint or journal, and then
processes the recovery queue. A held probe blocks younger replay when ordering
could matter. A consumer may explicitly declare an independent recovery lane;
the base semantics does not infer independence.

Each request has a finite replay budget; process recovery attempts are separated
by bounded backoff. Replay exhaustion terminates affected requests with an
operational error while the public transport remains owned by the host. The
host itself does not exit for an ordinary worker fault.

### Public Cancellation

Cancellation is advisory in MCP. A cancellation notification is routed to the
worker generation that owns the invocation. It does not change execution
knowledge. Only a terminal worker outcome establishes completion. If the worker
dies after cancellation, normal repetition law applies.

The host never kills a worker merely to accelerate cancellation of a
`AtMostOnce` invocation.

## Safety Invariants

Every reachable state satisfies the following invariants.

### I1. Unique Authority

At most one generation is active. Candidates and draining incumbents may
coexist as processes, but only the active generation receives unfenced new
invocations.

### I2. Immutable Invocation

`id`, `seq`, `method`, `payload`, effect contract, operation key, and causal
parent of an invocation never change after acceptance.

### I3. Generation Pinning

Every dispatched attempt, response, cancellation, progress notification, and
nested request is associated with exactly one generation. A terminal response
from any other generation is rejected.

### I4. Single Terminal Outcome

For every invocation `i`, the public trace contains at most one `reply(i, v)`.

### I5. Authorized Repetition

If the trace contains attempts `n` and `n+1` for `i`, the contract is
`ReplaySafe`, `Deduplicated(k)`, or `ProbeRequired(q)` with `SafeToRetry`
evidence. Attempt numbers strictly increase and remain bounded.

### I6. Single-Attempt Dispatch

For `AtMostOnce`, the trace contains at most one `dispatch(_, i, _)`.

### I7. Planned Effect Preservation

Rollover never terminates a worker that owns an in-flight invocation or scoped
obligation. A planned release change therefore neither cancels nor duplicates
an accepted effect.

### I8. Candidate Opacity

Before activation, a candidate emits no public business result, notification,
nested request, or external business effect. Its allowed actions are bounded
handshake, catalog construction, health checks declared effect-free, and
session-only restoration.

### I9. Session Prefix Consistency

The authoritative journal or checkpoint represents a prefix of successful
public session transitions. Candidate restoration is atomic. Activation occurs
only after the candidate represents the same prefix as the incumbent at the
fence.

### I10. Catalog Coherence

Every invocation dispatched after activation is classified against the active
generation's public catalog. A changed catalog becomes authoritative only at
activation, after a list-change notification and either a matching refresh
request or expiry of the finite refresh grace. A stale client call is rejected
or classified by the authoritative catalog; it is never dispatched under the
retired catalog's effect law.

### I11. Boundedness

Frames, pending invocations, queued invocations, journal bytes, checkpoints,
candidate count, callbacks, replay attempts, timers, and retained workers have
finite configured bounds. Exhaustion rejects explicitly; it never overwrites
live state.

### I12. Resource Ownership

Every host-created process, pipe, task, socket, temporary file, timer, and
candidate is owned by one RAII guard until transferred or reaped. No transition
drops the last owner of a live resource without executing its release law.

## Proof Sketches

### Theorem 1. At-Most-One Terminal Response

`I4` holds in every reachable state.

**Sketch.** Initially no invocation is terminal. The sole rule emitting
`reply(i, v)` requires nonterminal pending state and atomically replaces it with
`Terminal(v)`. No rule transitions out of `Terminal`. Worker responses are
accepted only when their rewritten ID maps to the currently pending invocation
and owning generation. Duplicate, late, and foreign responses therefore have
no emitting transition. Induction over host transitions proves the claim.

### Theorem 2. At-Most-Once Dispatch for Single-Attempt Effects

For every `i` with repetition `AtMostOnce`, the trace contains at most one
dispatch.

**Sketch.** The first dispatch changes `Accepted` to `Dispatched(g, 0)` before
the pipe write. Normal completion makes it terminal. Worker loss makes it
`OutcomeUnknown`; the only applicable recovery rule emits
`AmbiguousOutcome` and makes it terminal. Planned rollover does not alter the
invocation state or dispatch it to the candidate. There is no transition from
`Dispatched`, `OutcomeUnknown`, or `Terminal` to another dispatch for this
contract. Induction gives the result.

This is a host-level theorem. It assumes one worker dispatch does not itself
duplicate the business effect internally.

### Theorem 3. No Rollover Smothering

Suppose a planned rollover begins while invocation `i` is dispatched to `g`.
If `g` remains alive and business execution produces terminal value `v`, the
rollover neither kills `i` nor redirects it, and the host may emit
`reply(i, v)` before terminating `g`.

**Sketch.** The drain rule pins every pre-fence invocation to `g`. `I7` forbids
termination while such an invocation is in flight. `I3` sends its terminal
response only through `g`'s mapping. Activation requires the pre-fence set to
be empty, which occurs only after the response transition or an unplanned fault
transition. Therefore planned rollover cannot remove the execution path.

### Theorem 4. Generation-Serial History

Every public business invocation can be placed wholly before or wholly after
one activation point; no invocation is split across generations.

**Sketch.** Fence `κ` partitions accepted sequence numbers. Pre-fence attempts
remain pinned to `g`; post-fence invocations remain queued. Activation is
enabled only after the pre-fence pending set and pinned obligations are empty.
After activation, the queue dispatches only to `h`. `I3` excludes cross-
generation completion. Hence activation is a linearization point for the
generation change.

### Theorem 5. Bad Candidates Cannot Displace the Incumbent

Failure in verification, spawn, private initialization, catalog validation,
readiness, compatibility, or session restoration leaves `g` authoritative.

**Sketch.** No preparation rule writes `active`. The sole rule that changes
`active` is `activate(g, h)`, whose premises include all validations and atomic
restoration. Every earlier failure transition destroys only candidate-owned
state and returns to `Serving(g)`. Candidate opacity prevents a failed
candidate from having public business effects. Therefore authority and public
history remain with `g`.

### Theorem 6. Replay Respects the Declared Effect Law

Every second attempt is authorized by its immutable contract and, where
required, explicit domain evidence.

**Sketch.** Classification precedes first dispatch and is immutable by `I2`.
Worker loss is the only base transition to `OutcomeUnknown`. The recovery rule
case-splits on that stored contract. The `AtMostOnce` case has no replay
edge; `ProbeRequired` has a replay edge only from `SafeToRetry`; the other cases
carry their key and bounded attempt counter unchanged. No process-recovery or
rollover transition grants replay authority. Thus `I5` is inductive.

### Theorem 7. Session Restoration Preserves the Successful Prefix

Assume every journaled transition changes only `Σ`, journal compaction preserves
its sequential denotation, and checkpoint restore is atomic and version-
compatible. At activation, the candidate session state denotes the same
successful public transition prefix as the incumbent at the fence.

**Sketch.** A transition enters the journal only after a successful worker
response and before its public response. Thus the journal is a prefix of
successful public transitions. The fence stops further incumbent transitions
before restoration. Sequential replay, or atomic checkpoint restore, produces
the same `Σ` by premise. Failure activates nothing. Success activates only
after the equality witness. Therefore the candidate begins from the same
logical prefix.

### Theorem 8. Public Transport Survives Business-Worker Faults

An ordinary worker exit cannot by itself close public stdin or stdout.

**Sketch.** The host, not the worker, owns both public descriptors. Workers own
only private pipes. Worker exit closes private endpoints and generates a host
event; the recovery transition retains the public owners. RAII cleanup reaps
the worker without dropping public descriptors. Hence business failure is not
a public transport close.

This theorem does not cover host process death, parent death, kernel failure,
or explicit public shutdown.

### Theorem 9. Resource Non-Leakage

Assume every concrete resource constructor immediately returns an owning guard
whose destructor is total and idempotent. Then every finite host trace leaves
only resources reachable from live abstract state.

**Sketch.** By induction. Constructors add both resource and owner to the same
transition. Transfer moves the unique owner into successor state. Every state-
removing transition runs or drops the owner. Idempotence covers stacked error
paths. No rule creates an unowned resource. Therefore an unreachable resource
cannot remain after transition completion.

## Liveness

Safety is unconditional with respect to scheduling; liveness is not. We state
its premises rather than conceal them.

Assume:

- `A1` the host process and public transport remain alive;
- `A2` enabled host tasks are fairly scheduled;
- `A3` process creation, pipe I/O, and channel reads eventually return;
- `A4` the release channel eventually stops changing long enough to prepare
  one valid candidate;
- `A5` a healthy business worker eventually answers every admitted bounded
  request or terminates;
- `A6` restart budgets admit a healthy worker after finitely many failures;
- `A7` every required probe eventually returns a decisive answer;
- `A8` every generation-pinned invocation or obligation eventually terminates
  or is lawfully cancelled.

### Theorem 10. Request Termination

Under `A1`–`A7`, every accepted response-bound invocation eventually receives
one terminal public outcome.

**Sketch.** A queued invocation advances under fair scheduling and finite older
work. A healthy first attempt completes by `A5`. On loss, `AtMostOnce`
terminates immediately with ambiguity; repeatable or deduplicated work reaches
a healthy retry by `A6`; probe-required work becomes decidable by `A7` and
either completes from evidence, retries, or terminates unknown. Bounded budgets
turn infinite recovery attempts into a terminal operational error. `I4`
preserves uniqueness.

### Theorem 11. Rollover Progress

Under `A1`–`A5` and `A8`, a selected valid compatible candidate eventually
becomes active.

**Sketch.** Preparation terminates and yields a ready candidate. Catalog
refresh either arrives or its finite grace expires. The fence admits no new
work to the incumbent. The finite pre-fence set and pinned obligations terminate
by `A5` and `A8`. Bounded state restoration terminates by `A3`. Activation is
then continuously enabled and occurs by fair scheduling.

### Theorem 12. Worker-Recovery Progress

Under `A1`–`A7`, worker loss does not permanently prevent later independent
requests from completing, except where an unresolved earlier probe is required
to preserve order.

**Sketch.** Recovery either starts a healthy worker within budget or closes the
recovery epoch with explicit errors. Rejected at-most-once invocations leave
pending state. Replayable work is finite and ordered. A probe blocks only until
`A7`. The host then returns to serving. Without `A7`, preserving effect order
and permitting younger dependent work are incompatible; the base machine
chooses safety.

## Impossibility Boundaries

### Exactly Once

No host separated from the effect authority by a fallible transport can infer
exactly-once execution from request and response messages alone. Consider a
worker that commits an external effect and dies before its response reaches the
host. The host state is indistinguishable from a worker that died before
commit. Retrying violates at-most-once in the first history; refusing retry
violates at-least-once in the second. Exactly once therefore requires a durable
deduplication key, transactional status probe, or atomic coupling at the effect
authority.

`libmcp` never claims exactly once without such a premise.

### Unconditional Rollover Liveness

Suppose an incumbent owns a nonterminating `AtMostOnce`, generation-pinned
effect. Killing it violates planned effect preservation. Running a successor
concurrently violates generation seriality or the pin. Waiting violates
rollover liveness. No protocol satisfies all three. The runtime preserves the
effect and incumbent; rollover liveness is conditional on `A8`.

### Host Crash Transparency

MCP over inherited stdio gives Codex no reconnection handshake after the server
process dies. Therefore no in-process library can preserve that transport
across arbitrary host death. The architecture minimizes this trusted failure
domain by moving business code and fragile dependencies into workers. Planned
host replacement requires a separate descriptor-handoff protocol; absent that
protocol, a host-runtime upgrade remains one of the rare restart boundaries.

### Compensation Transparency

An effect followed by its inverse is not generally observationally equivalent
to no effect: intermediate observers, audit logs, billing, messages, and
physical actions may distinguish them. Compensation supports recovery policy,
not semantic erasure, unless the effect domain itself proves the stronger
equivalence.

## MCP Projection

The abstract contract is encoded in tool metadata under a reserved `libmcp`
namespace. Public standard annotations remain intact; private metadata is
stripped from the catalog returned to clients.

```json
{
  "annotations": {
    "readOnlyHint": false,
    "idempotentHint": false
  },
  "_meta": {
    "io.libmcp/effect": {
      "recovery": { "kind": "at_most_once" },
      "state": { "kind": "stateless" }
    }
  }
}
```

Business code owns durable keys, probes, checkpoints, and live obligations.
The generic stdio supervisor has no domain probe or checkpoint adapter. It
therefore refines `ProbeRequired` to `AtMostOnce` and `Checkpointed` to
`GenerationPinned`; direct kernel consumers may supply the missing evidence.
Conservative defaults are:

- standard read-only list/get/read requests: replay-safe;
- tool with `readOnlyHint = true`: replay-safe only if no contradictory private
  contract exists;
- tool with `idempotentHint = true`: replay-safe only under the MCP
  idempotence claim;
- every other tool: at-most-once and stateless;
- notification: at-most-once;
- unknown custom method: at-most-once.

Private metadata can reduce replay authority freely. Increasing authority over
the conservative interpretation requires a complete, valid contract.

## Concrete Refinement Obligations

An implementation conforms only if it supplies these witnesses.

| Abstract object | Concrete witness |
|---|---|
| immutable invocation | sealed parsed frame plus stored classification |
| sequence order | monotone checked counter |
| dispatch boundary | state transition before first private-pipe write |
| generation pin | request-ID map names worker incarnation |
| unique authority | one active-generation field changed only by activation |
| candidate opacity | private IDs, suppressed output, rejected nested calls |
| terminal uniqueness | pending-map removal and tombstone/late-response rejection |
| journal prefix | update after successful worker response, before public reply |
| atomic restore | construct candidate state off-path, activate only on success |
| catalog coherence | canonical public digest, notification, finite grace, and activation point |
| boundedness | checked capacities and deadlines for every collection/resource |
| RAII | owning process/pipe/temp/timer guards with explicit transfer |

The acceptance harness must exercise histories, not branch trivia. Its minimum
adversarial matrix is:

1. invalid candidate at every preparation boundary retains the incumbent;
2. a long at-most-once invocation completes on the incumbent during
   rollover and is never dispatched to the candidate;
3. worker death before any dispatch permits first dispatch elsewhere;
4. worker death after dispatch replays only repeatable or durably keyed work;
5. at-most-once loss yields one ambiguity error and later calls remain live;
6. probe-required work either waits for decisive evidence or receives the
   stricter at-most-once treatment;
7. changed catalog activates after refresh or bounded grace without dispatching
   stale calls under the retired effect law;
8. identical catalog cuts over silently at a quiescent fence;
9. session restoration preserves the successful transition prefix;
10. cancellation and nested requests remain generation-pinned;
11. crash loops exhaust bounded budgets without closing public transport;
12. every exit path reaps children and removes temporary runtime artifacts.

Passing examples does not prove the model complete. Any new concrete transition
must identify its abstract rule or extend this document first.

## Research Relation

Shi, Zhang, and Cui's [*A Programming Paradigm for Spatiotemporal
Composability*](https://github.com/cordiverse/paper) is methodological precedent
for stating recovery laws and their hypotheses before implementation. MCP
effects are often noninvertible, so this machine uses the smaller operational
taxonomy above. It does not import that paper's effect calculus.
