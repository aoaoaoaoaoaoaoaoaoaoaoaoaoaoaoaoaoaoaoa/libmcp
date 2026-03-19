# Retrofit

Use this when the MCP already exists and cannot simply be reimagined from
scratch.

## Retrofit Order

1. Separate session ownership from fragile execution.
2. Define typed replay contracts and typed faults.
3. Replace ad hoc backend dumps with porcelain-by-default output.
4. Add health, telemetry, and recovery tests.
5. Only then promise hot rollout or stronger operational guarantees.

## Specific Warnings

- Do not add retries before replay legality is explicit.
- Do not hide routing bugs behind warm-up masking.
- Do not call a worker self-healing if the host itself cannot roll forward.
- Do not let the canonical skill drift away from the actual library contract.

## Doctrine

The retrofit is complete only when:

- the hard posture lives in code
- the model UX doctrine is visible at the tool surface
- the skill, spec, and implementation agree
