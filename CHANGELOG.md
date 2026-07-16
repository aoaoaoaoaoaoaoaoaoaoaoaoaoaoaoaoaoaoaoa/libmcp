# Changelog

## Unreleased

The workspace is now on the `2.0.0-alpha.1` development line. The next release
will enforce the machine-grade continuity and presentation contracts in
`docs/spec.md` before declaring stability.

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
