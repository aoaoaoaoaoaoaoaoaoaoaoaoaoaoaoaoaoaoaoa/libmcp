# Checklist

Use this checklist when reviewing a `libmcp` consumer.

- Does a stable host own the public session?
- Is the public session backed by the shared host-session kernel rather than
  ad hoc initialize/reexec glue?
- Is worker fragility isolated behind an explicit replay policy?
- Are replay contracts typed and local to the request surface?
- Are faults typed and connected to recovery semantics?
- Do nontrivial tools default to porcelain output?
- Are library render helpers used where bespoke porcelain has not yet been
  justified?
- Is structured JSON still available where exact consumers need it?
- Are inputs normalized where the semantics are still unambiguous?
- Are health and telemetry available?
- Is event telemetry append-only and useful for postmortem analysis?
- Does the recovery test matrix cover the failure modes actually observed?
- Is the installed `$mcp-bootstrap` skill sourced from this repository?
