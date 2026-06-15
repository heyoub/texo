//! Compile-fail tests for typestate guarantees.

#[test]
fn typestate_guards() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}
