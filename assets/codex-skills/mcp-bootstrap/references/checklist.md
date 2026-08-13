# Checklist

Use this checklist when reviewing a `libmcp` consumer.

- Does a stable host own the public session?
- Are the public session, host epoch, and worker generation named separately?
- Is the public session backed by the shared host-session kernel rather than
  ad hoc initialize/reexec glue?
- Is public initialization driven only by exact observed client events?
- Does every routed invocation receive a replay contract before dispatch?
- Does the host distinguish `NotDispatched`, `InFlight`, `Completed`, and
  `OutcomeUnknown`?
- Do queued client frames remain distinct from replay-authorized dispatches?
- Are replay attempts charged only on actual redispatch?
- Does `ProbeRequired` remain blocked until explicit evidence arrives?
- Does `NeverReplay` produce an explicit ambiguous-outcome error?
- Are process recovery and request disposition independent?
- Are faults typed, with process hints kept advisory?
- Do tool surfaces cross an explicit projection boundary rather than serializing
  raw domain/store structs directly?
- Do nontrivial tools default to porcelain output?
- Does each result emit exactly one representation, with no porcelain plus
  `structuredContent` or structured JSON duplicated as text?
- Does porcelain avoid JSON unless the data is irreducibly tree-shaped?
- Do tabular results render as compact tables with headers, `|` separators, and
  unquoted scalar cells?
- Are `render` and `detail` treated as orthogonal controls?
- Does `detail=concise` return an actual summary rather than the full payload?
- Are the projection traits or derive-macro happy path used on hot surfaces,
  with generic JSON fallbacks reserved for explicit escape hatches?
- Are library render helpers used where bespoke porcelain has not yet been
  justified?
- Is structured JSON still available where exact consumers need it?
- Are inputs normalized where the semantics are still unambiguous?
- Does health distinguish host lifecycle from worker handshake phase?
- Do session-scoped health and telemetry survive churn and coordinated reexec?
- Are terminal errors distinct from nonterminal recovery faults?
- Is event telemetry append-only, record-atomic under concurrent writers, and
  explicit about flush durability?
- Are reexec snapshots private, bounded, one-shot, version-exact, and validated
  before hydration?
- Does the consumer run directly with no managed-release environment?
- If managed rollout is supported, is the channel atomic, the release immutable,
  and the executable digest verified?
- Does the incumbent retain the session until the successor has hydrated and
  crossed a private readiness barrier?
- Is process replacement deferred while a timed frame reader owns buffered
  input?
- Do promotion and rollback enforce declared state-epoch compatibility?
- Does the unit-test layer avoid one change-shaped test per incident, branch,
  or matrix row?
- Does the recovery matrix cover loss before dispatch, during execution, after
  completion, probe barriers, queue exhaustion, and corrupt reexec handoff?
- Is the installed `$mcp-bootstrap` skill sourced from this repository?
