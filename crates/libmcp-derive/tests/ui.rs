//! Compile-time conformance tests for the public derive surface.

use libmcp as _;
use libmcp_derive as _;
use proc_macro_crate as _;
use proc_macro2 as _;
use quote as _;
use syn as _;

#[test]
fn accepts_downstream_projection_types() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/pass/*.rs");
}

#[test]
fn rejects_unsupported_input_shapes() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/fail/*.rs");
}
