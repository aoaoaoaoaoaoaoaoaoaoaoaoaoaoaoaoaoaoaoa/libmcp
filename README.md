# libmcp

Industrial MCP hardening spine for Rust.

`libmcp` provides the reusable control-plane pieces used by hardened MCP
servers:

- typed replay contracts and operational faults
- JSON-RPC request identity, frame IO, and tool-call metadata helpers
- a bounded host-session kernel for worker churn
- a generic stable-stdio supervisor over disposable full-MCP workers
- verified release channels and readiness-gated blue/green rollover
- shared health snapshots, telemetry snapshots, and append-only JSONL events
- porcelain-by-default rendering, projection traits, and derive macros
- model-facing normalization helpers
- testkit assertions for projection doctrine and telemetry fixtures

The source repository also owns the `$mcp-bootstrap` Codex skill under
`assets/codex-skills`. That skill is library collateral: it tracks the same
operational doctrine, but it is not part of the runtime crate API.

## Status

`libmcp` `2.2` implements the contract in `docs/spec.md`; the abstract machine
and proof sketches are in `formal_semantics.md`. The workspace contains
`libmcp`, `libmcp-derive`, and `libmcp-testkit`.

The continuity spine distinguishes public sessions, host epochs, worker
generations, execution knowledge, and per-invocation replay contracts. Process
recovery never confers replay authority; exact downstream conformance tests
exercise churn, probe barriers, bounds, snapshots, and initialization.

`run_supervised` owns public stdio and proxies an ordinary full MCP executable
over private stdio. The worker remains directly runnable without a release
depot. Custom transports may compose the lower-level kernel.

Managed rollout does not alter the standalone contract. With no
`LIBMCP_RELEASE_CHANNEL` and `LIBMCP_RELEASE_GENERATION` environment, a consumer
runs directly and observes only atomic replacement of its own executable path.
An external builder may publish immutable releases and set those variables, but
no daemon, registry, or release store is required to build or run a consumer.

## Layout

- `docs/spec.md`: normative design and versioning contract
- `formal_semantics.md`: abstract machine, laws, and proof sketches
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
