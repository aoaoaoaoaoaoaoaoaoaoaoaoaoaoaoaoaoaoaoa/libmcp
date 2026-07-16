# Retrofit

Use this when the MCP already exists and cannot simply be reimagined from
scratch.

## Retrofit Order

1. Separate the public session and host epoch from fragile worker generations.
2. Journal execution knowledge and assign a typed replay contract to every
   invocation before dispatch.
3. Define typed operational faults with optional process hints; never encode
   request replay authority in a fault.
4. Replace ad hoc backend dumps with porcelain-by-default output.
   Make `render` and `detail` orthogonal before you start bikeshedding prose.
   Do not rebrand pretty-printed JSON as porcelain. If the data is tabular,
   render a compact `|`-separated table with headers and unquoted scalar cells;
   reserve JSON-shaped porcelain for data that is genuinely tree-shaped.
5. Make session-scoped health and telemetry survive worker churn and
   coordinated reexec.
6. Add recovery tests across pre-dispatch, uncertain execution, completion,
   probe barriers, capacity exhaustion, and corrupt handoff.
7. Only then promise hot rollout or stronger operational guarantees.

When `libmcp` is in play, prefer its host-session kernel, projection traits,
derive macros, render helpers, health payloads, and telemetry log over
consumer-local copies.

## Specific Warnings

- Do not add retries before replay legality is explicit.
- Do not let a restart, recovery hint, queue pop, or missing response confer
  replay authority.
- Do not synthesize public initialize or initialized events.
- Do not hide routing bugs behind warm-up masking.
- Do not call a worker self-healing if the host itself cannot roll forward.
- Do not let the canonical skill drift away from the actual library contract.

## Doctrine

The retrofit is complete only when:

- the hard posture lives in code
- the model UX doctrine is visible at the tool surface
- the skill, spec, and implementation agree
