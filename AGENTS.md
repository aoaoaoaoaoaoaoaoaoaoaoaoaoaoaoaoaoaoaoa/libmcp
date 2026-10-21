libmcp: industrial MCP hardening spine for Rust; source of `$mcp-bootstrap`

## invariants
- Release-channel runtime optional: every consumer stays a normal standalone MCP executable when managed-release env vars are absent
- Every `vX.Y.Z` tag ships on crates.io first: `cargo publish --workspace --locked`, then push the signed tag; the Foundry publisher refuses a tag crates.io lacks
