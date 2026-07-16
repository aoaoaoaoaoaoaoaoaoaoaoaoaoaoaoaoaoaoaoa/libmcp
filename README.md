# libmcp

Industrial MCP hardening spine for Rust.

`libmcp` provides the reusable control-plane pieces used by hardened MCP
servers:

- typed replay contracts and operational faults
- JSON-RPC request identity, frame IO, and tool-call metadata helpers
- a durable host-session kernel for worker churn and host self-reexec handoff
- shared health snapshots, telemetry snapshots, and append-only JSONL events
- porcelain-by-default rendering, projection traits, and derive macros
- model-facing normalization helpers
- testkit assertions for projection doctrine and telemetry fixtures

The source repository also owns the `$mcp-bootstrap` Codex skill under
`assets/codex-skills`. That skill is library collateral: it tracks the same
operational doctrine, but it is not part of the runtime crate API.

## Status

`libmcp` is developing the machine-grade `2.0` contract defined in
`docs/spec.md`. The workspace currently identifies itself as `2.0.0-alpha.1`
until every normative guarantee has executable conformance evidence.

The last published release is `1.1.0`. The public workspace contains
`libmcp`, `libmcp-derive`, and `libmcp-testkit`.

This release does not prescribe a single worker transport or ship runtime
adapter crates. Consumers keep domain tools, backend routing, and worker
transport local while reusing the shared replay, health, telemetry, host-session,
rendering, projection, and normalization primitives.

## Layout

- `docs/spec.md`: normative design and versioning contract
- `crates/libmcp`: public library crate
- `crates/libmcp-derive`: derive macros for projection traits
- `crates/libmcp-testkit`: shared hardening fixtures and assertions
- `assets/codex-skills/mcp-bootstrap`: canonical skill source
- `scripts/link-codex-skills`: installs the repo-owned skill into `~/.codex`

## Development

For local changes, keep the full workspace clean:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```
