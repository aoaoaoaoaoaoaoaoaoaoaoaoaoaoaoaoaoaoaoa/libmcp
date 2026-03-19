# Changelog

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
- proof by integration into `adequate_rust_mcp`

Explicitly excluded from `1.0.0`:

- `fidget_spinner`
- forced runtime adapter crates
- backend-specific warm-up or routing policy beyond what the first consumer
  still keeps locally
