//! Compile-time conformance tests for the public derive surface.

use libmcp as _;
use libmcp_derive as _;
use proc_macro2 as _;
use quote as _;
use syn as _;

#[test]
fn rejects_unsupported_input_shapes() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/fail/*.rs");
}
