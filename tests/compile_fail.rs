//! Compile-fail tests for transition machine guarantees.

#[test]
fn transition_machine_guards() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}
